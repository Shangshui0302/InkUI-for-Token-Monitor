package dev.tokenmonitor.eink;

import java.io.IOException;

/** Synthetic record storage only; no files, Android runtime, Keystore, or real credentials. */
public final class ConnectionStoreMigrationTest {
    public static void main(String[] args) {
        ConnectionStore.Config hub = new ConnectionStore.Config("http://synthetic-hub", "synthetic-secret", 37);
        MemoryStorage legacy = new MemoryStorage(hub, true);
        ConnectionStore migrated = new ConnectionStore(legacy);
        checkHub(migrated.snapshot());
        check(legacy.writes == 1 && !legacy.needsRewrite, "legacy record rewritten once");
        checkHub(legacy.record);
        checkHub(new ConnectionStore(legacy).snapshot());
        check(legacy.writes == 1, "canonical record is not rewritten on next launch");

        MemoryStorage unavailable = new MemoryStorage(hub, true);
        unavailable.failWrite = true;
        ConnectionStore retained = new ConnectionStore(unavailable);
        checkHub(retained.snapshot());
        checkHub(unavailable.record);
        check(unavailable.needsRewrite, "failed migration keeps old record available for retry");
        unavailable.failWrite = false;
        checkHub(new ConnectionStore(unavailable).snapshot());
        checkHub(unavailable.record);
        check(unavailable.writes == 2 && !unavailable.needsRewrite, "next launch retries migration");

        MemoryStorage unreadable = new MemoryStorage(hub, true);
        unreadable.failRead = true;
        ConnectionStore blocked = new ConnectionStore(unreadable);
        check(blocked.snapshot().hubUrl.isEmpty() && blocked.snapshot().hubSecret.isEmpty(), "no partial credentials after read failure");
        check(unreadable.writes == 0, "failed read must not overwrite disk record");
        checkHub(unreadable.record);
        unreadable.failRead = false;
        checkHub(new ConnectionStore(unreadable).snapshot());

        MemoryStorage empty = new MemoryStorage(null, false);
        check(new ConnectionStore(empty).snapshot().hubUrl.isEmpty(), "fresh install stays unconfigured");
        check(empty.writes == 0, "fresh install does not create an empty record");
        System.out.println("Connection store migration checks passed (synthetic storage; no Keystore or disk proof)");
    }

    private static final class MemoryStorage implements ConnectionStore.Storage {
        ConnectionStore.Config record;
        boolean needsRewrite, failWrite, failRead;
        int writes;
        MemoryStorage(ConnectionStore.Config record, boolean needsRewrite) {
            this.record = record; this.needsRewrite = needsRewrite;
        }
        @Override public ConnectionStore.StoredRecord read() throws Exception {
            if (failRead) throw new IOException("synthetic read failure");
            return record == null ? null : new ConnectionStore.StoredRecord(record, needsRewrite);
        }
        @Override public void write(ConnectionStore.Config config) throws Exception {
            writes++;
            if (failWrite) throw new IOException("synthetic write failure");
            record = config; needsRewrite = false;
        }
    }
    private static void checkHub(ConnectionStore.Config config) {
        check("http://synthetic-hub".equals(config.hubUrl), "Hub URL preserved");
        check("synthetic-secret".equals(config.hubSecret), "Hub secret preserved");
        check(config.revision == 37, "migration preserves revision");
    }
    private static void check(boolean value, String message) { if (!value) throw new AssertionError(message); }
}
