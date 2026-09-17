//! SQLite-backed persistence for public device-group state. Private keys have
//! no representation in this store and must remain in a platform key store.
use crate::membership::{MembershipBundle, MembershipError, validate_bundle};
use crate::types::{AddressCandidate, DeviceProfile, GroupProfile};
use nekodex_domain::{
    checkpoint::MembershipCheckpoint, signing::SignedMembershipEvent, MembershipEvent,
};
use nekodex_protocol::{Cursor, EventEnvelope, ProcessGeneration, Project};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{de::DeserializeOwned, Serialize};
use std::{
    path::Path,
    sync::{Mutex, MutexGuard},
    time::{SystemTime, UNIX_EPOCH},
};
use thiserror::Error;

pub const SCHEMA_VERSION: i64 = 6;
pub const SOURCE_GENERATION_CACHE_KEY: &str = "meta:source-process-generation";
pub const OFFLINE_CACHE_MAX_BYTES: u64 = 100 * 1024 * 1024;
#[derive(Debug, Error)]
pub enum StorageError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("database schema version {0} is newer than supported version {SCHEMA_VERSION}")]
    NewerSchema(i64),
    #[error("storage mutex was poisoned")]
    Poisoned,
    #[error("conflicting membership event: {0}")]
    Conflict(String),
    #[error("group {0} is no longer active")]
    MissingGroup(String),
    #[error("cursor {0} is outside SQLite's signed integer range")]
    CursorOutOfRange(u64),
    #[error("stored cursor cannot be negative: {0}")]
    NegativeCursor(i64),
    #[error("invalid membership bundle: {0}")]
    InvalidMembership(#[from] MembershipError),
}
pub type Result<T> = std::result::Result<T, StorageError>;

fn cache_access_clock() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros()
        .min(i64::MAX as u128) as i64
}

#[derive(Clone, Debug, PartialEq)]
pub struct CacheEntry<T> {
    pub source: String,
    pub key: String,
    pub value: T,
    pub observed_at_unix_seconds: i64,
}

pub struct Storage {
    connection: Mutex<Connection>,
}
impl Storage {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::from_connection(Connection::open(path)?)
    }
    pub fn open_in_memory() -> Result<Self> {
        Self::from_connection(Connection::open_in_memory()?)
    }
    fn from_connection(c: Connection) -> Result<Self> {
        c.busy_timeout(std::time::Duration::from_secs(5))?;
        c.execute_batch("PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL;")?;
        let s = Self {
            connection: Mutex::new(c),
        };
        s.migrate()?;
        Ok(s)
    }
    fn connection(&self) -> Result<MutexGuard<'_, Connection>> {
        self.connection.lock().map_err(|_| StorageError::Poisoned)
    }
    pub fn schema_version(&self) -> Result<i64> {
        Ok(self
            .connection()?
            .query_row("PRAGMA user_version", [], |r| r.get(0))?)
    }
    fn migrate(&self) -> Result<()> {
        let mut c = self.connection()?;
        let v: i64 = c.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if v > SCHEMA_VERSION {
            return Err(StorageError::NewerSchema(v));
        }
        if v == SCHEMA_VERSION {
            return Ok(());
        }
        let tx = c.transaction()?;
        if v < 1 {
            tx.execute_batch("\
CREATE TABLE IF NOT EXISTS groups(group_id TEXT PRIMARY KEY,payload TEXT NOT NULL);\
CREATE TABLE IF NOT EXISTS devices(device_id TEXT PRIMARY KEY,group_id TEXT NOT NULL,payload TEXT NOT NULL);\
CREATE TABLE IF NOT EXISTS membership_events(group_id TEXT NOT NULL,event_id TEXT NOT NULL,payload TEXT NOT NULL,PRIMARY KEY(group_id,event_id));\
CREATE TABLE IF NOT EXISTS membership_checkpoints(group_id TEXT PRIMARY KEY,payload TEXT NOT NULL);\
CREATE TABLE IF NOT EXISTS address_candidates(cache_source TEXT NOT NULL,cache_key TEXT NOT NULL,payload TEXT NOT NULL,PRIMARY KEY(cache_source,cache_key));\
CREATE TABLE IF NOT EXISTS cache_entries(cache_source TEXT NOT NULL,cache_key TEXT NOT NULL,payload TEXT NOT NULL,observed_at INTEGER NOT NULL DEFAULT 0,PRIMARY KEY(cache_source,cache_key));\
CREATE TABLE IF NOT EXISTS event_cursors(source TEXT PRIMARY KEY,cursor INTEGER NOT NULL);")?;
        }
        if v < 2 {
            tx.execute_batch(
                "CREATE TABLE IF NOT EXISTS projects(project_id TEXT PRIMARY KEY,source_device_id TEXT NOT NULL,payload TEXT NOT NULL);",
            )?;
        }
        if v < 4 {
            tx.execute_batch(
                "CREATE TABLE IF NOT EXISTS source_events(\
                    source_device_id TEXT NOT NULL,\
                    process_generation INTEGER NOT NULL,\
                    cursor INTEGER NOT NULL,\
                    observed_at INTEGER NOT NULL,\
                    payload TEXT NOT NULL,\
                    PRIMARY KEY(source_device_id,process_generation,cursor)\
                );\
                CREATE INDEX IF NOT EXISTS source_events_observed \
                    ON source_events(observed_at,source_device_id,process_generation,cursor);",
            )?;
        }
        if v < 5 {
            tx.execute_batch("ALTER TABLE cache_entries ADD COLUMN accessed_at INTEGER NOT NULL DEFAULT 0; UPDATE cache_entries SET accessed_at=observed_at;")?;
        }
        if v < 6 {
            tx.execute_batch(
                "DROP TABLE IF EXISTS commands;\
                 DROP TABLE IF EXISTS drafts;\
                 DROP TABLE IF EXISTS managed_threads;",
            )?;
        }
        tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        tx.commit()?;
        Ok(())
    }
    pub fn put_group(&self, v: &GroupProfile) -> Result<()> {
        self.put_one("groups", "group_id", &v.group_id, v)
    }
    pub fn get_group(&self, id: &str) -> Result<Option<GroupProfile>> {
        self.get("SELECT payload FROM groups WHERE group_id=?1", params![id])
    }
    pub fn groups(&self) -> Result<Vec<GroupProfile>> {
        self.list("SELECT payload FROM groups ORDER BY group_id", [])
    }
    pub fn put_device(&self, group: &str, v: &DeviceProfile) -> Result<()> {
        let p = serde_json::to_string(v)?;
        self.connection()?.execute("INSERT INTO devices(device_id,group_id,payload)VALUES(?1,?2,?3) ON CONFLICT(device_id) DO UPDATE SET group_id=excluded.group_id,payload=excluded.payload",params![v.device_id,group,p])?;
        Ok(())
    }
    /// Persist the group, local profile, and validated membership bundle as a
    /// single unit so a failed create/join cannot leave a half-written group.
    pub fn put_initial_group_state(
        &self,
        group: &GroupProfile,
        profile: &DeviceProfile,
        bundle: &MembershipBundle,
    ) -> Result<()> {
        validate_bundle(bundle)?;
        let group_payload = serde_json::to_string(group)?;
        let profile_payload = serde_json::to_string(profile)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction()?;
        transaction.execute(
            "INSERT INTO groups(group_id,payload)VALUES(?1,?2) \
             ON CONFLICT(group_id) DO UPDATE SET payload=excluded.payload",
            params![group.group_id, group_payload],
        )?;
        transaction.execute(
            "INSERT INTO devices(device_id,group_id,payload)VALUES(?1,?2,?3) \
             ON CONFLICT(device_id) DO UPDATE SET group_id=excluded.group_id,payload=excluded.payload",
            params![profile.device_id, group.group_id, profile_payload],
        )?;
        put_membership_bundle_transaction(&transaction, bundle)?;
        transaction.commit()?;
        Ok(())
    }
    pub fn get_device(&self, id: &str) -> Result<Option<DeviceProfile>> {
        self.get(
            "SELECT payload FROM devices WHERE device_id=?1",
            params![id],
        )
    }
    pub fn devices(&self, group: &str) -> Result<Vec<DeviceProfile>> {
        self.list(
            "SELECT payload FROM devices WHERE group_id=?1 ORDER BY device_id",
            params![group],
        )
    }
    pub fn put_membership_event(&self, v: &SignedMembershipEvent) -> Result<()> {
        let id = event_id(v);
        let p = serde_json::to_string(v)?;
        let mut c = self.connection()?;
        let tx = c.transaction()?;
        let old: Option<String> = tx
            .query_row(
                "SELECT payload FROM membership_events WHERE group_id=?1 AND event_id=?2",
                params![v.group_id, id],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(old) = old {
            if old != p {
                return Err(StorageError::Conflict(id.into()));
            }
        } else {
            tx.execute(
                "INSERT INTO membership_events(group_id,event_id,payload)VALUES(?1,?2,?3)",
                params![v.group_id, id, p],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
    pub fn membership_events(&self, g: &str) -> Result<Vec<SignedMembershipEvent>> {
        let c = self.connection()?;
        let mut s =
            c.prepare("SELECT payload FROM membership_events WHERE group_id=?1 ORDER BY event_id")?;
        let rows = s.query_map(params![g], |r| r.get::<_, String>(0))?;
        rows.map(|r| Ok(serde_json::from_str(&r?)?)).collect()
    }
    pub fn put_membership_checkpoint(&self, v: &MembershipCheckpoint) -> Result<()> {
        self.put_one("membership_checkpoints", "group_id", &v.group_id, v)
    }

    /// Persist a previously validated complete membership bundle atomically.
    /// Existing event ids may only be replayed with byte-identical contents;
    /// the checkpoint advances in the same transaction as the new events.
    pub fn put_membership_bundle(&self, bundle: &MembershipBundle) -> Result<()> {
        validate_bundle(bundle)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction()?;
        put_membership_bundle_transaction(&transaction, bundle)?;
        transaction.commit()?;
        Ok(())
    }
    pub fn membership_checkpoint(&self, g: &str) -> Result<Option<MembershipCheckpoint>> {
        self.get(
            "SELECT payload FROM membership_checkpoints WHERE group_id=?1",
            params![g],
        )
    }
    pub fn put_address_candidate(
        &self,
        source: &str,
        key: &str,
        v: &AddressCandidate,
    ) -> Result<()> {
        self.put_cache_table("address_candidates", source, key, v)
    }
    pub fn address_candidate(&self, source: &str, key: &str) -> Result<Option<AddressCandidate>> {
        self.get2(
            "SELECT payload FROM address_candidates WHERE cache_source=?1 AND cache_key=?2",
            params![source, key],
        )
    }
    pub fn address_candidates(&self, source: &str, limit: usize) -> Result<Vec<AddressCandidate>> {
        self.list_limited("SELECT payload FROM address_candidates WHERE cache_source=?1 ORDER BY cache_key LIMIT ?2",params![source,limit.min(1000) as i64])
    }
    pub fn delete_address_candidate(&self, source: &str, key: &str) -> Result<bool> {
        Ok(self.connection()?.execute(
            "DELETE FROM address_candidates WHERE cache_source=?1 AND cache_key=?2",
            params![source, key],
        )? != 0)
    }
    pub fn put_cache<T: Serialize>(&self, source: &str, key: &str, v: &T) -> Result<()> {
        self.put_cache_with_observed(source, key, v, 0)
    }
    pub fn put_cache_with_observed<T: Serialize>(
        &self,
        source: &str,
        key: &str,
        v: &T,
        observed: i64,
    ) -> Result<()> {
        let p = serde_json::to_string(v)?;
        let accessed = cache_access_clock();
        self.connection()?.execute("INSERT INTO cache_entries(cache_source,cache_key,payload,observed_at,accessed_at)VALUES(?1,?2,?3,?4,?5) ON CONFLICT(cache_source,cache_key) DO UPDATE SET payload=excluded.payload,observed_at=excluded.observed_at,accessed_at=excluded.accessed_at",params![source,key,p,observed,accessed])?;
        Ok(())
    }
    pub fn clear_cache_source(&self, source: &str) -> Result<usize> {
        Ok(self.connection()?.execute(
            "DELETE FROM cache_entries WHERE cache_source=?1",
            params![source],
        )?)
    }
    /// Atomically invalidate request cache from an older source process and
    /// install the generation marker for the replacement process.
    pub fn begin_source_generation(
        &self,
        source: &str,
        generation: ProcessGeneration,
        observed: i64,
    ) -> Result<()> {
        let payload = serde_json::to_string(&generation)?;
        let accessed = cache_access_clock();
        let mut connection = self.connection()?;
        let transaction = connection.transaction()?;
        transaction.execute(
            "DELETE FROM cache_entries WHERE cache_source=?1",
            params![source],
        )?;
        transaction.execute(
            "INSERT INTO cache_entries(cache_source,cache_key,payload,observed_at,accessed_at) VALUES(?1,?2,?3,?4,?5)",
            params![source, SOURCE_GENERATION_CACHE_KEY, payload, observed, accessed],
        )?;
        transaction.commit()?;
        Ok(())
    }
    /// Remove monitoring data learned from a device that is no longer an
    /// active group member. Membership history itself remains authoritative.
    pub fn clear_observed_source(&self, source: &str) -> Result<()> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction()?;
        transaction.execute(
            "DELETE FROM cache_entries WHERE cache_source=?1",
            params![source],
        )?;
        transaction.execute(
            "DELETE FROM source_events WHERE source_device_id=?1",
            params![source],
        )?;
        transaction.execute("DELETE FROM event_cursors WHERE source=?1", params![source])?;
        transaction.execute(
            "DELETE FROM address_candidates WHERE cache_key=?1",
            params![source],
        )?;
        transaction.execute(
            "DELETE FROM projects WHERE source_device_id=?1",
            params![source],
        )?;
        transaction.commit()?;
        Ok(())
    }
    fn put_cache_table<T: Serialize>(
        &self,
        table: &str,
        source: &str,
        key: &str,
        v: &T,
    ) -> Result<()> {
        let p = serde_json::to_string(v)?;
        let q = format!(
            "INSERT INTO {table}(cache_source,cache_key,payload)VALUES(?1,?2,?3) ON CONFLICT(cache_source,cache_key) DO UPDATE SET payload=excluded.payload"
        );
        self.connection()?.execute(&q, params![source, key, p])?;
        Ok(())
    }
    pub fn cache<T: DeserializeOwned>(&self, source: &str, key: &str) -> Result<Option<T>> {
        let c = self.connection()?;
        let payload: Option<String> = c
            .query_row(
                "SELECT payload FROM cache_entries WHERE cache_source=?1 AND cache_key=?2",
                params![source, key],
                |r| r.get(0),
            )
            .optional()?;
        if payload.is_some() {
            c.execute(
                "UPDATE cache_entries SET accessed_at=?3 WHERE cache_source=?1 AND cache_key=?2",
                params![source, key, cache_access_clock()],
            )?;
        }
        payload.map(|p| Ok(serde_json::from_str(&p)?)).transpose()
    }
    pub fn cached_entries<T: DeserializeOwned>(
        &self,
        source: &str,
        limit: usize,
    ) -> Result<Vec<CacheEntry<T>>> {
        let c = self.connection()?;
        let mut s=c.prepare("SELECT cache_key,payload,observed_at FROM cache_entries WHERE cache_source=?1 ORDER BY observed_at DESC,cache_key LIMIT ?2")?;
        let rows = s.query_map(params![source, limit.min(1000) as i64], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
            ))
        })?;
        let entries: Vec<CacheEntry<T>> = rows
            .map(|r| {
                let (key, payload, observed) = r?;
                Ok(CacheEntry {
                    source: source.into(),
                    key,
                    value: serde_json::from_str(&payload)?,
                    observed_at_unix_seconds: observed,
                })
            })
            .collect::<Result<_>>()?;
        drop(s);
        let accessed = cache_access_clock();
        for entry in &entries {
            c.execute(
                "UPDATE cache_entries SET accessed_at=?3 WHERE cache_source=?1 AND cache_key=?2",
                params![source, entry.key, accessed],
            )?;
        }
        Ok(entries)
    }
    pub fn cached_entry<T: DeserializeOwned>(
        &self,
        source: &str,
        key: &str,
    ) -> Result<Option<CacheEntry<T>>> {
        let c = self.connection()?;
        let row: Option<(String, i64)> = c
            .query_row(
                "SELECT payload,observed_at FROM cache_entries WHERE cache_source=?1 AND cache_key=?2",
                params![source, key],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        if row.is_some() {
            c.execute(
                "UPDATE cache_entries SET accessed_at=?3 WHERE cache_source=?1 AND cache_key=?2",
                params![source, key, cache_access_clock()],
            )?;
        }
        row.map(|(payload, observed_at_unix_seconds)| {
            Ok(CacheEntry {
                source: source.to_owned(),
                key: key.to_owned(),
                value: serde_json::from_str(&payload)?,
                observed_at_unix_seconds,
            })
        })
        .transpose()
    }
    /// Approximate persisted cache footprint for one source, in bytes.
    pub fn cache_bytes(&self, source: &str) -> Result<u64> {
        let value: i64 = self.connection()?.query_row(
            "SELECT COALESCE(SUM(length(CAST(cache_key AS BLOB))+length(CAST(payload AS BLOB))),0) FROM cache_entries WHERE cache_source=?1",
            params![source], |row| row.get(0))?;
        Ok(value.max(0) as u64)
    }
    /// Remove persisted monitoring data until the evictable global cache is
    /// within `max_bytes`. Request entries are removed by access time, then
    /// the oldest activity events if more space is still required.
    /// Source-generation markers stay pinned so a later offline read cannot
    /// silently cross a source process boundary. Membership and projects live
    /// in separate tables and are never evicted.
    pub fn evict_cache_to(&self, max_bytes: u64) -> Result<Vec<String>> {
        let mut c = self.connection()?;
        let tx = c.transaction()?;
        let request_bytes: i64 = tx.query_row(
            "SELECT COALESCE(SUM(length(CAST(cache_key AS BLOB))+length(CAST(payload AS BLOB))),0) FROM cache_entries WHERE cache_key != ?1",
            params![SOURCE_GENERATION_CACHE_KEY],
            |row| row.get(0),
        )?;
        let event_bytes: i64 = tx.query_row(
            "SELECT COALESCE(SUM(length(CAST(source_device_id AS BLOB))+length(CAST(payload AS BLOB))+16),0) FROM source_events",
            [],
            |row| row.get(0),
        )?;
        let mut bytes = request_bytes.saturating_add(event_bytes);
        if bytes.max(0) as u64 <= max_bytes {
            tx.commit()?;
            return Ok(Vec::new());
        }
        let mut s = tx.prepare("SELECT cache_source,cache_key,length(CAST(cache_key AS BLOB))+length(CAST(payload AS BLOB)) FROM cache_entries WHERE cache_key != ?1 ORDER BY accessed_at ASC, rowid ASC")?;
        let keys: Vec<(String, String, i64)> = s
            .query_map(params![SOURCE_GENERATION_CACHE_KEY], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
            .collect::<std::result::Result<_, _>>()?;
        drop(s);
        let mut removed = Vec::new();
        for (source, key, size) in keys {
            if bytes.max(0) as u64 <= max_bytes {
                break;
            }
            tx.execute(
                "DELETE FROM cache_entries WHERE cache_source=?1 AND cache_key=?2",
                params![source, key],
            )?;
            bytes -= size;
            removed.push(format!("{}:{}", source, key));
        }
        if bytes.max(0) as u64 > max_bytes {
            let mut statement = tx.prepare(
                "SELECT source_device_id,process_generation,cursor,\
                 length(CAST(source_device_id AS BLOB))+length(CAST(payload AS BLOB))+16 \
                 FROM source_events ORDER BY observed_at ASC,rowid ASC",
            )?;
            let events: Vec<(String, i64, i64, i64)> = statement
                .query_map([], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
                })?
                .collect::<std::result::Result<_, _>>()?;
            drop(statement);
            for (source, generation, cursor, size) in events {
                if bytes.max(0) as u64 <= max_bytes {
                    break;
                }
                tx.execute(
                    "DELETE FROM source_events WHERE source_device_id=?1 AND process_generation=?2 AND cursor=?3",
                    params![source, generation, cursor],
                )?;
                bytes -= size;
                removed.push(format!("event:{source}:{generation}:{cursor}"));
            }
        }
        tx.commit()?;
        Ok(removed)
    }
    pub fn put_project(&self, project: &Project) -> Result<()> {
        let payload = serde_json::to_string(project)?;
        self.connection()?.execute(
            "INSERT INTO projects(project_id,source_device_id,payload)VALUES(?1,?2,?3) \
             ON CONFLICT(project_id) DO UPDATE SET source_device_id=excluded.source_device_id,payload=excluded.payload",
            params![project.project_id, project.source_device_id, payload],
        )?;
        Ok(())
    }
    pub fn projects(&self, source_device_id: &str) -> Result<Vec<Project>> {
        self.list(
            "SELECT payload FROM projects WHERE source_device_id=?1 ORDER BY project_id",
            params![source_device_id],
        )
    }
    pub fn delete_project(&self, source_device_id: &str, project_id: &str) -> Result<bool> {
        Ok(self.connection()?.execute(
            "DELETE FROM projects WHERE source_device_id=?1 AND project_id=?2",
            params![source_device_id, project_id],
        )? != 0)
    }
    pub fn put_cursor(&self, source: &str, cursor: u64) -> Result<()> {
        let cursor = i64::try_from(cursor).map_err(|_| StorageError::CursorOutOfRange(cursor))?;
        self.connection()?.execute("INSERT INTO event_cursors(source,cursor)VALUES(?1,?2) ON CONFLICT(source) DO UPDATE SET cursor=excluded.cursor",params![source,cursor])?;
        Ok(())
    }
    pub fn cursor(&self, source: &str) -> Result<Option<u64>> {
        let value: Option<i64> = self
            .connection()?
            .query_row(
                "SELECT cursor FROM event_cursors WHERE source=?1",
                params![source],
                |r| r.get(0),
            )
            .optional()?;
        value
            .map(|v| {
                if v < 0 {
                    Err(StorageError::NegativeCursor(v))
                } else {
                    Ok(v as u64)
                }
            })
            .transpose()
    }
    /// Persist an authenticated source event and its latest cursor together.
    /// Each source keeps only its most recent bounded history.
    pub fn put_source_event(&self, event: &EventEnvelope, observed_at: i64) -> Result<()> {
        let generation = i64::try_from(event.process_generation.0)
            .map_err(|_| StorageError::CursorOutOfRange(event.process_generation.0))?;
        let cursor = i64::try_from(event.cursor.0)
            .map_err(|_| StorageError::CursorOutOfRange(event.cursor.0))?;
        let payload = serde_json::to_string(event)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction()?;
        let old: Option<String> = transaction
            .query_row(
                "SELECT payload FROM source_events WHERE source_device_id=?1 AND process_generation=?2 AND cursor=?3",
                params![event.source_device_id, generation, cursor],
                |row| row.get(0),
            )
            .optional()?;
        match old {
            Some(old) if old != payload => {
                return Err(StorageError::Conflict(format!(
                    "event:{}:{}:{}",
                    event.source_device_id, generation, cursor
                )));
            }
            Some(_) => {}
            None => {
                transaction.execute(
                    "INSERT INTO source_events(source_device_id,process_generation,cursor,observed_at,payload) VALUES(?1,?2,?3,?4,?5)",
                    params![event.source_device_id, generation, cursor, observed_at, payload],
                )?;
            }
        }
        transaction.execute(
            "INSERT INTO event_cursors(source,cursor) VALUES(?1,?2) ON CONFLICT(source) DO UPDATE SET cursor=excluded.cursor",
            params![event.source_device_id, cursor],
        )?;
        transaction.execute(
            "DELETE FROM source_events WHERE rowid IN (\
                SELECT rowid FROM source_events WHERE source_device_id=?1 \
                ORDER BY observed_at DESC,process_generation DESC,cursor DESC LIMIT -1 OFFSET 1000\
            )",
            params![event.source_device_id],
        )?;
        transaction.commit()?;
        drop(connection);
        self.evict_cache_to(OFFLINE_CACHE_MAX_BYTES)?;
        Ok(())
    }

    pub fn source_events(&self, limit: usize) -> Result<Vec<EventEnvelope>> {
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT payload FROM (\
                SELECT payload,observed_at,source_device_id,process_generation,cursor \
                FROM source_events \
                ORDER BY observed_at DESC,source_device_id DESC,process_generation DESC,cursor DESC \
                LIMIT ?1\
            ) ORDER BY observed_at,source_device_id,process_generation,cursor",
        )?;
        let rows = statement.query_map(params![limit.min(1000) as i64], |row| {
            row.get::<_, String>(0)
        })?;
        rows.map(|row| Ok(serde_json::from_str(&row?)?)).collect()
    }

    pub fn source_events_for(
        &self,
        source_device_id: &str,
        limit: usize,
    ) -> Result<Vec<EventEnvelope>> {
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT payload FROM (\
                SELECT payload,rowid \
                FROM source_events WHERE source_device_id=?1 \
                ORDER BY rowid DESC LIMIT ?2\
            ) ORDER BY rowid",
        )?;
        let rows = statement.query_map(
            params![source_device_id, limit.min(1000) as i64],
            |row| row.get::<_, String>(0),
        )?;
        rows.map(|row| Ok(serde_json::from_str(&row?)?)).collect()
    }

    /// Return the most recently persisted source position. Keeping the
    /// generation with the cursor prevents a restarted source from being
    /// resumed with a cursor that belonged to an older process. Insertion
    /// order is authoritative here: wall-clock timestamps may repeat or move
    /// backwards, and a late durable write must still become the resume point.
    pub fn latest_source_position(
        &self,
        source_device_id: &str,
    ) -> Result<Option<(ProcessGeneration, Cursor)>> {
        let value: Option<(i64, i64)> = self
            .connection()?
            .query_row(
                "SELECT process_generation,cursor FROM source_events \
                 WHERE source_device_id=?1 \
                 ORDER BY rowid DESC LIMIT 1",
                params![source_device_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        value
            .map(|(generation, cursor)| {
                if generation < 0 {
                    return Err(StorageError::NegativeCursor(generation));
                }
                if cursor < 0 {
                    return Err(StorageError::NegativeCursor(cursor));
                }
                Ok((ProcessGeneration(generation as u64), Cursor(cursor as u64)))
            })
            .transpose()
    }
    /// Clear the single-group public state while preserving the device identity,
    /// which is owned by the platform secret store rather than this database.
    pub fn reset_group_state(&self) -> Result<()> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction()?;
        transaction.execute_batch(
            "DELETE FROM source_events;\
             DELETE FROM event_cursors;\
             DELETE FROM cache_entries;\
             DELETE FROM address_candidates;\
             DELETE FROM projects;\
             DELETE FROM membership_checkpoints;\
             DELETE FROM membership_events;\
             DELETE FROM devices;\
             DELETE FROM groups;",
        )?;
        transaction.commit()?;
        Ok(())
    }
    fn put_one<T: Serialize>(&self, table: &str, col: &str, key: &str, v: &T) -> Result<()> {
        let p = serde_json::to_string(v)?;
        let q = format!(
            "INSERT INTO {table}({col},payload)VALUES(?1,?2) ON CONFLICT({col}) DO UPDATE SET payload=excluded.payload"
        );
        self.connection()?.execute(&q, params![key, p])?;
        Ok(())
    }
    fn get<T: DeserializeOwned>(&self, q: &str, p: impl rusqlite::Params) -> Result<Option<T>> {
        self.get2(q, p)
    }
    fn list<T: DeserializeOwned>(&self, q: &str, p: impl rusqlite::Params) -> Result<Vec<T>> {
        self.list_limited(q, p)
    }
    fn list_limited<T: DeserializeOwned>(
        &self,
        q: &str,
        p: impl rusqlite::Params,
    ) -> Result<Vec<T>> {
        let c = self.connection()?;
        let mut s = c.prepare(q)?;
        let rows = s.query_map(p, |r| r.get::<_, String>(0))?;
        rows.map(|r| Ok(serde_json::from_str(&r?)?)).collect()
    }
    fn get2<T: DeserializeOwned>(&self, q: &str, p: impl rusqlite::Params) -> Result<Option<T>> {
        let payload = self
            .connection()?
            .query_row(q, p, |r| r.get::<_, String>(0))
            .optional()?;
        payload
            .map(|s| serde_json::from_str(&s).map_err(StorageError::from))
            .transpose()
    }
}

fn put_membership_bundle_transaction(
    transaction: &rusqlite::Transaction<'_>,
    bundle: &MembershipBundle,
) -> Result<()> {
    validate_bundle(bundle)?;
    let group_exists: bool = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM groups WHERE group_id=?1)",
        params![bundle.group_id],
        |row| row.get(0),
    )?;
    if !group_exists {
        return Err(StorageError::MissingGroup(bundle.group_id.clone()));
    }
    for event in &bundle.events {
        let id = event_id(event);
        let payload = serde_json::to_string(event)?;
        let old: Option<String> = transaction
            .query_row(
                "SELECT payload FROM membership_events WHERE group_id=?1 AND event_id=?2",
                params![bundle.group_id, id],
                |row| row.get(0),
            )
            .optional()?;
        match old {
            Some(old) if old != payload => return Err(StorageError::Conflict(id.into())),
            Some(_) => {}
            None => {
                transaction.execute(
                    "INSERT INTO membership_events(group_id,event_id,payload)VALUES(?1,?2,?3)",
                    params![bundle.group_id, id, payload],
                )?;
            }
        }
    }
    let checkpoint = serde_json::to_string(&bundle.checkpoint)?;
    transaction.execute(
        "INSERT INTO membership_checkpoints(group_id,payload)VALUES(?1,?2) \
         ON CONFLICT(group_id) DO UPDATE SET payload=excluded.payload",
        params![bundle.group_id, checkpoint],
    )?;
    Ok(())
}
fn event_id(v: &SignedMembershipEvent) -> &str {
    match &v.event {
        MembershipEvent::GroupCreated(e) => &e.event_id,
        MembershipEvent::Added(e) => &e.event_id,
        MembershipEvent::Revoked(e) => &e.event_id,
        MembershipEvent::Renamed(e) => &e.event_id,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::membership::GroupMembership;
    use nekodex_domain::{GroupCreated, MembershipEvent};
    use tempfile::tempdir;
    fn g() -> GroupProfile {
        GroupProfile {
            group_id: "g".into(),
            name: "G".into(),
            local_device_id: "d".into(),
        }
    }
    fn d() -> DeviceProfile {
        DeviceProfile {
            device_id: "d".into(),
            name: "D".into(),
            signing_public_key: [1; 32],
            noise_public_key: [2; 32],
            codex_capable: true,
            protocol_version: 1,
        }
    }
    fn e(id: &str) -> SignedMembershipEvent {
        let k = ed25519_dalek::SigningKey::from_bytes(&[7; 32]);
        SignedMembershipEvent::sign(
            "g",
            MembershipEvent::GroupCreated(GroupCreated {
                event_id: id.into(),
                group_id: "g".into(),
                member_id: nekodex_domain::signing::member_id_for_key(&k.verifying_key()),
                profile: None,
            }),
            &k,
        )
        .unwrap()
    }
    fn table_exists(storage: &Storage, name: &str) -> bool {
        storage
            .connection()
            .unwrap()
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
                params![name],
                |row| row.get(0),
            )
            .unwrap()
    }
    #[test]
    fn reopen_and_version() {
        let directory = tempdir().unwrap();
        let x = directory.path().join("x.db");
        {
            let s = Storage::open(&x).unwrap();
            assert_eq!(s.schema_version().unwrap(), SCHEMA_VERSION);
            s.put_group(&g()).unwrap();
            s.put_device("g", &d()).unwrap();
        }
        let s = Storage::open(&x).unwrap();
        assert_eq!(s.get_group("g").unwrap(), Some(g()));
        assert_eq!(s.get_device("d").unwrap(), Some(d()));
        for table in ["commands", "drafts", "managed_threads"] {
            assert!(!table_exists(&s, table));
        }
    }
    #[test]
    fn schema_v6_removes_legacy_control_tables() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("legacy.db");
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE commands(request_id TEXT PRIMARY KEY,payload TEXT NOT NULL);\
                 CREATE TABLE drafts(draft_id TEXT PRIMARY KEY,payload TEXT NOT NULL);\
                 CREATE TABLE managed_threads(source_device_id TEXT NOT NULL,thread_id TEXT NOT NULL);\
                 PRAGMA user_version=5;",
            )
            .unwrap();
        drop(connection);

        let storage = Storage::open(&path).unwrap();
        assert_eq!(storage.schema_version().unwrap(), SCHEMA_VERSION);
        for table in ["commands", "drafts", "managed_threads"] {
            assert!(!table_exists(&storage, table));
        }
    }
    #[test]
    fn event_order_and_idempotency() {
        let s = Storage::open_in_memory().unwrap();
        s.put_membership_event(&e("b")).unwrap();
        s.put_membership_event(&e("a")).unwrap();
        s.put_membership_event(&e("a")).unwrap();
        assert_eq!(
            s.membership_events("g")
                .unwrap()
                .iter()
                .map(event_id)
                .collect::<Vec<_>>(),
            vec!["a", "b"]
        );
    }
    #[test]
    fn membership_bundle_cannot_recreate_a_removed_group() {
        let storage = Storage::open_in_memory().unwrap();
        let key = ed25519_dalek::SigningKey::from_bytes(&[9; 32]);
        let membership = GroupMembership::create("removed", "genesis", &key).unwrap();

        let error = storage
            .put_membership_bundle(&membership.export_bundle())
            .unwrap_err();

        assert!(matches!(error, StorageError::MissingGroup(group) if group == "removed"));
        assert!(storage.membership_events("removed").unwrap().is_empty());
        assert!(storage.membership_checkpoint("removed").unwrap().is_none());
    }

    #[test]
    fn oversized_membership_bundle_is_rejected_before_sqlite_write() {
        let storage = Storage::open_in_memory().unwrap();
        storage.put_group(&g()).unwrap();
        let bundle = MembershipBundle {
            group_id: "g".into(),
            events: vec![e("root"); crate::membership::MAX_MEMBERSHIP_EVENTS + 1],
            checkpoint: MembershipCheckpoint::from_event_ids("g", ["root"]).unwrap(),
        };
        assert!(matches!(
            storage.put_membership_bundle(&bundle),
            Err(StorageError::InvalidMembership(MembershipError::TooManyEvents))
        ));
        assert!(storage.membership_events("g").unwrap().is_empty());
        assert!(storage.membership_checkpoint("g").unwrap().is_none());
    }

    #[test]
    fn source_isolated_cache() {
        let s = Storage::open_in_memory().unwrap();
        s.put_cache("a", "k", &serde_json::json!({"v":1})).unwrap();
        s.put_cache("b", "k", &serde_json::json!({"v":2})).unwrap();
        assert_eq!(
            s.cache::<serde_json::Value>("a", "k").unwrap(),
            Some(serde_json::json!({"v":1}))
        );
        assert_eq!(
            s.cache::<serde_json::Value>("b", "k").unwrap(),
            Some(serde_json::json!({"v":2}))
        );
    }
    #[test]
    fn clearing_one_cache_source_preserves_other_sources() {
        let s = Storage::open_in_memory().unwrap();
        s.put_cache("a", "one", &serde_json::json!(1)).unwrap();
        s.put_cache("a", "two", &serde_json::json!(2)).unwrap();
        s.put_cache("b", "keep", &serde_json::json!(3)).unwrap();
        assert_eq!(s.clear_cache_source("a").unwrap(), 2);
        assert_eq!(s.cache::<serde_json::Value>("a", "one").unwrap(), None);
        assert_eq!(
            s.cache::<serde_json::Value>("b", "keep").unwrap(),
            Some(serde_json::json!(3))
        );
    }

    #[test]
    fn beginning_source_generation_replaces_only_that_sources_cache() {
        let s = Storage::open_in_memory().unwrap();
        s.put_cache("source", "old", &serde_json::json!(1)).unwrap();
        s.put_cache("other", "keep", &serde_json::json!(2)).unwrap();

        s.begin_source_generation("source", ProcessGeneration(42), 123)
            .unwrap();

        assert!(
            s.cache::<serde_json::Value>("source", "old")
                .unwrap()
                .is_none()
        );
        assert_eq!(
            s.cache::<ProcessGeneration>("source", SOURCE_GENERATION_CACHE_KEY)
                .unwrap(),
            Some(ProcessGeneration(42))
        );
        assert_eq!(
            s.cache::<serde_json::Value>("other", "keep").unwrap(),
            Some(serde_json::json!(2))
        );
    }

    #[test]
    fn clearing_observed_source_removes_only_revoked_device_data() {
        let s = Storage::open_in_memory().unwrap();
        s.put_cache("revoked", "request", &serde_json::json!(1)).unwrap();
        s.put_cache("active", "request", &serde_json::json!(2)).unwrap();
        let event = EventEnvelope {
            version: nekodex_protocol::ProtocolVersion::V1,
            source_device_id: "revoked".into(),
            process_generation: ProcessGeneration(1),
            cursor: Cursor(1),
            event: nekodex_protocol::Event::Snapshot {
                devices: Vec::new(),
                threads: Vec::new(),
            },
        };
        s.put_source_event(&event, 1).unwrap();
        s.connection()
            .unwrap()
            .execute(
                "INSERT INTO address_candidates(cache_source,cache_key,payload) VALUES('group','revoked','{}')",
                [],
            )
            .unwrap();

        s.clear_observed_source("revoked").unwrap();

        assert!(s.cache::<serde_json::Value>("revoked", "request").unwrap().is_none());
        assert!(s.cache::<serde_json::Value>("active", "request").unwrap().is_some());
        assert!(s.source_events_for("revoked", 10).unwrap().is_empty());
        assert!(s.cursor("revoked").unwrap().is_none());
        let candidates: i64 = s
            .connection()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM address_candidates WHERE cache_key='revoked'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(candidates, 0);
    }
    #[test]
    fn cursor_round_trip() {
        let s = Storage::open_in_memory().unwrap();
        s.put_cursor("s", 42).unwrap();
        assert_eq!(s.cursor("s").unwrap(), Some(42));
    }
    #[test]
    fn source_events_survive_reopen_and_deduplicate() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("events.db");
        let event = EventEnvelope {
            version: nekodex_protocol::ProtocolVersion::V1,
            source_device_id: "source".into(),
            process_generation: nekodex_protocol::ProcessGeneration(7),
            cursor: nekodex_protocol::Cursor(9),
            event: nekodex_protocol::Event::Snapshot {
                devices: Vec::new(),
                threads: Vec::new(),
            },
        };
        {
            let storage = Storage::open(&path).unwrap();
            storage.put_source_event(&event, 100).unwrap();
            storage.put_source_event(&event, 100).unwrap();
            let mut later = event.clone();
            later.process_generation = ProcessGeneration(6);
            later.cursor = Cursor(1);
            // The wall clock moved backwards, but this is still the newest
            // durable position because it was inserted last.
            storage.put_source_event(&later, 50).unwrap();
            let mut other = event.clone();
            other.source_device_id = "other".into();
            storage.put_source_event(&other, 300).unwrap();
        }
        let storage = Storage::open(&path).unwrap();
        assert_eq!(storage.source_events(10).unwrap().len(), 3);
        assert_eq!(storage.source_events_for("source", 10).unwrap().len(), 2);
        assert_eq!(storage.source_events_for("other", 10).unwrap(), vec![{
            let mut other = event.clone();
            other.source_device_id = "other".into();
            other
        }]);
        assert_eq!(storage.cursor("source").unwrap(), Some(1));
        assert_eq!(
            storage.latest_source_position("source").unwrap(),
            Some((ProcessGeneration(6), Cursor(1)))
        );
    }
    #[test]
    fn cursor_boundaries_and_negative_values_are_rejected() {
        let s = Storage::open_in_memory().unwrap();
        s.put_cursor("s", i64::MAX as u64).unwrap();
        assert_eq!(s.cursor("s").unwrap(), Some(i64::MAX as u64));
        assert!(matches!(
            s.put_cursor("too-large", i64::MAX as u64 + 1),
            Err(StorageError::CursorOutOfRange(_))
        ));
        s.connection()
            .unwrap()
            .execute("UPDATE event_cursors SET cursor=-1 WHERE source='s'", [])
            .unwrap();
        assert!(matches!(
            s.cursor("s"),
            Err(StorageError::NegativeCursor(-1))
        ));
    }
    #[test]
    fn bounded_lists_keep_source_and_observation_time() {
        let s = Storage::open_in_memory().unwrap();
        s.put_device("g", &d()).unwrap();
        assert_eq!(s.devices("g").unwrap(), vec![d()]);
        s.put_cache_with_observed("source", "old", &serde_json::json!(1), 10)
            .unwrap();
        s.put_cache_with_observed("source", "new", &serde_json::json!(2), 20)
            .unwrap();
        let entries = s.cached_entries::<serde_json::Value>("source", 1).unwrap();
        assert_eq!(entries[0].key, "new");
        assert_eq!(entries[0].observed_at_unix_seconds, 20);
    }

    #[test]
    fn cache_reads_refresh_global_lru_without_changing_observation() {
        let s = Storage::open_in_memory().unwrap();
        s.put_cache_with_observed("a", "keep", &"中文", 10).unwrap();
        s.put_cache_with_observed("b", "evict", &"old", 20).unwrap();
        s.connection()
            .unwrap()
            .execute("UPDATE cache_entries SET accessed_at=1", [])
            .unwrap();
        let entry = s.cached_entry::<String>("a", "keep").unwrap().unwrap();
        assert_eq!(entry.observed_at_unix_seconds, 10);
        let budget = s.cache_bytes("a").unwrap();
        assert_eq!(
            budget,
            ("keep".len() + serde_json::to_string("中文").unwrap().len()) as u64
        );
        assert_eq!(s.evict_cache_to(budget).unwrap(), vec!["b:evict"]);
        assert!(s.cache::<String>("a", "keep").unwrap().is_some());
    }

    #[test]
    fn cache_eviction_is_global() {
        let s = Storage::open_in_memory().unwrap();
        s.put_cache_with_observed("a", "old", &serde_json::json!("old"), 1)
            .unwrap();
        s.put_cache_with_observed("a", "new", &serde_json::json!("new"), 2)
            .unwrap();
        s.put_cache("b", "keep", &serde_json::json!("remote"))
            .unwrap();
        let removed = s.evict_cache_to(0).unwrap();
        assert_eq!(removed.len(), 3);
        assert!(s.cache::<serde_json::Value>("b", "keep").unwrap().is_none());
    }

    #[test]
    fn cache_eviction_includes_persisted_source_events() {
        let s = Storage::open_in_memory().unwrap();
        s.connection()
            .unwrap()
            .execute(
                "INSERT INTO source_events(source_device_id,process_generation,cursor,observed_at,payload) VALUES('source',1,2,3,'event-payload')",
                [],
            )
            .unwrap();

        assert_eq!(
            s.evict_cache_to(0).unwrap(),
            vec!["event:source:1:2"]
        );
        assert!(s.source_events_for("source", 10).unwrap().is_empty());
    }

    #[test]
    fn cache_eviction_preserves_source_generation_markers() {
        let s = Storage::open_in_memory().unwrap();
        s.put_cache("source", SOURCE_GENERATION_CACHE_KEY, &ProcessGeneration(7))
            .unwrap();
        s.put_cache("source", "request:data", &serde_json::json!({"value": 1}))
            .unwrap();

        assert_eq!(s.evict_cache_to(0).unwrap(), vec!["source:request:data"]);
        assert_eq!(
            s.cache::<ProcessGeneration>("source", SOURCE_GENERATION_CACHE_KEY)
                .unwrap(),
            Some(ProcessGeneration(7))
        );
    }
}
