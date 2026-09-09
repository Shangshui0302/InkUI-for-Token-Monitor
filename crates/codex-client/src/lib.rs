use codex_monitor_protocol::{
    AccountState, CollectorStatus, CreditBalance, DailyUsageBucket, RateLimitBucket,
    RateLimitWindow, SessionState, StateSnapshot, MAX_STATE_DAILY_USAGE,
};
use serde_json::{json, Map, Value};
use std::env;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::time::{timeout, Duration};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_APP_SERVER_LINE_BYTES: usize = 256 * 1024;

/// Small read-only JSON-RPC client for the local `codex app-server` process.
pub struct CodexCollector {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl CodexCollector {
    pub async fn spawn() -> Result<Self, String> {
        let binary = env::var("CODEX_MONITOR_CODEX_BIN").unwrap_or_else(|_| {
            if cfg!(windows) {
                "codex.cmd".to_string()
            } else {
                "codex".to_string()
            }
        });
        let mut child = Command::new(binary)
            .args(["app-server", "--listen", "stdio://"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|error| format!("start codex app-server: {error}"))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "codex app-server stdin is unavailable".to_string())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "codex app-server stdout is unavailable".to_string())?;
        let mut collector = Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            next_id: 1,
        };
        collector.initialize().await?;
        Ok(collector)
    }

    pub async fn snapshot(
        &mut self,
        host_id: &str,
        alias: &str,
        sequence: u64,
        captured_at: u64,
    ) -> Result<StateSnapshot, String> {
        let mut errors = Vec::new();
        let threads = match self
            .request(
                "thread/list",
                Some(json!({
                    "limit": 25,
                    "sortKey": "updated_at",
                    "sortDirection": "desc"
                })),
            )
            .await
        {
            Ok(value) => Some(parse_sessions(&value)),
            Err(error) => {
                errors.push(format!("thread/list: {error}"));
                None
            }
        };

        let account_result = match self
            .request("account/read", Some(json!({ "refreshToken": false })))
            .await
        {
            Ok(value) => Some(value),
            Err(error) => {
                errors.push(format!("account/read: {error}"));
                None
            }
        };
        let rate_limits = match self
            .request("account/rateLimits/read", Some(json!({})))
            .await
        {
            Ok(value) => Some(value),
            Err(error) => {
                errors.push(format!("account/rateLimits/read: {error}"));
                None
            }
        };
        let usage = match self.request("account/usage/read", Some(json!({}))).await {
            Ok(value) => Some(value),
            Err(error) => {
                errors.push(format!("account/usage/read: {error}"));
                None
            }
        };

        if threads.is_none() && account_result.is_none() && rate_limits.is_none() && usage.is_none()
        {
            return Err(errors.join("; "));
        }

        let account = merge_account(
            account_result.as_ref(),
            rate_limits.as_ref(),
            usage.as_ref(),
            captured_at,
        );
        let collector_status = if account
            .as_ref()
            .is_some_and(|value| value.requires_openai_auth && value.auth_mode.is_none())
        {
            CollectorStatus::AuthRequired
        } else if errors.is_empty() {
            CollectorStatus::Ready
        } else {
            CollectorStatus::Degraded
        };

        Ok(StateSnapshot {
            host_id: host_id.to_string(),
            alias: alias.to_string(),
            sequence,
            captured_at,
            collector_status,
            error: (!errors.is_empty()).then(|| truncate(&errors.join("; "), 1024)),
            sessions: threads.unwrap_or_default(),
            account,
        })
    }

    async fn initialize(&mut self) -> Result<(), String> {
        self.request(
            "initialize",
            Some(json!({
                "clientInfo": {
                    "name": "codex-monitor",
                    "title": "Codex Monitor",
                    "version": env!("CARGO_PKG_VERSION")
                }
            })),
        )
        .await?;
        self.send_notification("initialized", Some(json!({}))).await
    }

    async fn send_notification(
        &mut self,
        method: &str,
        params: Option<Value>,
    ) -> Result<(), String> {
        let mut message = Map::new();
        message.insert("method".to_string(), Value::String(method.to_string()));
        if let Some(params) = params {
            message.insert("params".to_string(), params);
        }
        self.write_message(Value::Object(message)).await
    }

    async fn request(&mut self, method: &str, params: Option<Value>) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let mut message = Map::new();
        message.insert("method".to_string(), Value::String(method.to_string()));
        message.insert("id".to_string(), Value::from(id));
        if let Some(params) = params {
            message.insert("params".to_string(), params);
        }
        self.write_message(Value::Object(message)).await?;

        timeout(REQUEST_TIMEOUT, self.read_response(id))
            .await
            .map_err(|_| format!("{method} timed out"))?
    }

    async fn write_message(&mut self, message: Value) -> Result<(), String> {
        let mut bytes = serde_json::to_vec(&message).map_err(|error| error.to_string())?;
        bytes.push(b'\n');
        self.stdin
            .write_all(&bytes)
            .await
            .map_err(|error| format!("write app-server request: {error}"))?;
        self.stdin
            .flush()
            .await
            .map_err(|error| format!("flush app-server request: {error}"))
    }

    async fn read_response(&mut self, expected_id: u64) -> Result<Value, String> {
        loop {
            let mut line = Vec::new();
            let read = self
                .stdout
                .read_until(b'\n', &mut line)
                .await
                .map_err(|error| format!("read app-server response: {error}"))?;
            if read == 0 {
                let status = self
                    .child
                    .try_wait()
                    .map_err(|error| format!("inspect app-server: {error}"))?;
                return Err(match status {
                    Some(status) => format!("app-server exited with {status}"),
                    None => "app-server closed its output".to_string(),
                });
            }
            if line.len() > MAX_APP_SERVER_LINE_BYTES {
                return Err("app-server response exceeds maximum size".to_string());
            }
            let value: Value = serde_json::from_slice(&line)
                .map_err(|error| format!("decode app-server response: {error}"))?;
            if value.get("id").and_then(Value::as_u64) != Some(expected_id) {
                // Notifications are expected on this stream. Requests initiated by the
                // server are not used by the read-only collector and are ignored here.
                continue;
            }
            if let Some(error) = value.get("error") {
                let message = error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown app-server error");
                return Err(truncate(message, 512));
            }
            return Ok(value.get("result").cloned().unwrap_or(Value::Null));
        }
    }
}

impl Drop for CodexCollector {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

fn parse_sessions(result: &Value) -> Vec<SessionState> {
    result
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let thread_id = item
                .get("id")
                .or_else(|| item.get("threadId"))
                .or_else(|| item.get("sessionId"))
                .and_then(Value::as_str)?;
            Some(SessionState {
                thread_id: truncate(thread_id, 128),
                title: item
                    .get("name")
                    .or_else(|| item.get("title"))
                    .and_then(Value::as_str)
                    .map(|value| truncate(value, 1024)),
                status: parse_status(item.get("status")),
                last_output_summary: item
                    .get("preview")
                    .or_else(|| item.get("summary"))
                    .and_then(Value::as_str)
                    .map(|value| truncate(value, 1024)),
                created_at: number(item.get("createdAt")),
                updated_at: number(item.get("updatedAt")),
            })
        })
        .collect()
}

fn parse_status(value: Option<&Value>) -> String {
    let Some(value) = value else {
        return "unknown".to_string();
    };
    if let Some(status) = value.as_str() {
        return normalize_status(status);
    }
    let Some(object) = value.as_object() else {
        return "unknown".to_string();
    };
    let status = object
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    if status == "active"
        && object
            .get("activeFlags")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .any(|flag| flag.to_ascii_lowercase().contains("wait"))
    {
        return "waiting".to_string();
    }
    normalize_status(status)
}

fn normalize_status(status: &str) -> String {
    match status.to_ascii_lowercase().as_str() {
        "active" | "running" => "active",
        "idle" => "idle",
        "notloaded" | "not_loaded" => "unknown",
        "waiting" | "wait" => "waiting",
        "error" | "systemerror" | "system_error" => "error",
        _ => "unknown",
    }
    .to_string()
}

fn merge_account(
    account_result: Option<&Value>,
    rate_limits_result: Option<&Value>,
    usage_result: Option<&Value>,
    observed_at: u64,
) -> Option<AccountState> {
    let account_value = account_result
        .and_then(|value| value.get("account"))
        .and_then(|value| (!value.is_null()).then_some(value));
    let requires_openai_auth = account_result
        .and_then(|value| value.get("requiresOpenaiAuth"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let (rate_limits, reset_credits_available) = rate_limits_result
        .map(parse_rate_limits)
        .unwrap_or_default();
    let usage_summary = usage_result.and_then(|value| value.get("summary"));
    let daily_usage = usage_result.map(parse_daily_usage).unwrap_or_default();
    let account_key = account_value.and_then(account_group_key);
    let auth_mode = account_value
        .and_then(|value| value.get("type"))
        .and_then(Value::as_str)
        .map(|value| truncate(value, 64));
    let plan_type = account_value
        .and_then(|value| value.get("planType"))
        .and_then(Value::as_str)
        .map(|value| truncate(value, 64))
        .or_else(|| {
            rate_limits
                .iter()
                .find_map(|bucket| bucket.plan_type.clone())
        });

    if account_value.is_none()
        && !requires_openai_auth
        && rate_limits.is_empty()
        && usage_summary.is_none()
        && daily_usage.is_empty()
    {
        return None;
    }
    Some(AccountState {
        account_key,
        auth_mode,
        plan_type,
        requires_openai_auth,
        rate_limits,
        reset_credits_available,
        usage_lifetime_tokens: usage_summary.and_then(|value| number(value.get("lifetimeTokens"))),
        usage_peak_daily_tokens: usage_summary
            .and_then(|value| number(value.get("peakDailyTokens"))),
        daily_usage,
        observed_at,
    })
}

fn parse_daily_usage(result: &Value) -> Vec<DailyUsageBucket> {
    let mut buckets = result
        .get("dailyUsageBuckets")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let start_date = item.get("startDate").and_then(Value::as_str)?;
            if !valid_date(start_date) {
                return None;
            }
            let tokens = item.get("tokens").and_then(Value::as_u64)?;
            Some(DailyUsageBucket {
                start_date: start_date.to_string(),
                tokens,
            })
        })
        .collect::<Vec<_>>();
    buckets.sort_by(|left, right| left.start_date.cmp(&right.start_date));
    buckets.dedup_by(|left, right| left.start_date == right.start_date);
    if buckets.len() > MAX_STATE_DAILY_USAGE {
        buckets = buckets.split_off(buckets.len() - MAX_STATE_DAILY_USAGE);
    }
    buckets
}

fn valid_date(value: &str) -> bool {
    if value.len() != 10 {
        return false;
    }
    let bytes = value.as_bytes();
    bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| matches!(index, 4 | 7) || byte.is_ascii_digit())
        && value[5..7]
            .parse::<u8>()
            .is_ok_and(|month| (1..=12).contains(&month))
        && value[8..10]
            .parse::<u8>()
            .is_ok_and(|day| (1..=31).contains(&day))
}

fn parse_rate_limits(result: &Value) -> (Vec<RateLimitBucket>, Option<u64>) {
    let mut buckets = result
        .get("rateLimitsByLimitId")
        .and_then(Value::as_object)
        .map(|values| {
            values
                .iter()
                .filter_map(|(limit_id, value)| parse_bucket(limit_id, value))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if buckets.is_empty() {
        if let Some(value) = result.get("rateLimits") {
            if let Some(limit_id) = value.get("limitId").and_then(Value::as_str) {
                if let Some(bucket) = parse_bucket(limit_id, value) {
                    buckets.push(bucket);
                }
            }
        }
    }
    buckets.sort_by(|left, right| left.limit_id.cmp(&right.limit_id));
    let reset_credits = result
        .get("rateLimitResetCredits")
        .and_then(|value| value.get("availableCount"))
        .and_then(Value::as_u64);
    (buckets, reset_credits)
}

fn parse_bucket(limit_id: &str, value: &Value) -> Option<RateLimitBucket> {
    Some(RateLimitBucket {
        limit_id: truncate(limit_id, 128),
        limit_name: value
            .get("limitName")
            .and_then(Value::as_str)
            .map(|item| truncate(item, 128)),
        primary: value.get("primary").and_then(parse_window),
        secondary: value.get("secondary").and_then(parse_window),
        credits: value.get("credits").and_then(parse_credits),
        plan_type: value
            .get("planType")
            .and_then(Value::as_str)
            .map(|item| truncate(item, 64)),
        rate_limit_reached_type: value
            .get("rateLimitReachedType")
            .and_then(Value::as_str)
            .map(|item| truncate(item, 64)),
    })
}

fn parse_window(value: &Value) -> Option<RateLimitWindow> {
    if !value.is_object() {
        return None;
    }
    Some(RateLimitWindow {
        used_percent: value.get("usedPercent").and_then(Value::as_f64),
        window_duration_mins: number(value.get("windowDurationMins")),
        resets_at: number(value.get("resetsAt")),
    })
}

fn parse_credits(value: &Value) -> Option<CreditBalance> {
    if !value.is_object() {
        return None;
    }
    Some(CreditBalance {
        has_credits: value.get("hasCredits").and_then(Value::as_bool),
        unlimited: value.get("unlimited").and_then(Value::as_bool),
        balance: value.get("balance").and_then(|item| {
            item.as_str()
                .map(str::to_string)
                .or_else(|| item.as_f64().map(|number| number.to_string()))
        }),
    })
}

fn account_group_key(value: &Value) -> Option<String> {
    ["accountId", "id", "email"]
        .iter()
        .find_map(|field| value.get(*field).and_then(Value::as_str))
        .map(|value| format!("fnv1a:{:016x}", fnv1a(value)))
}

fn fnv1a(value: &str) -> u64 {
    // ponytail: a compact non-secret grouping key; use a keyed hash if this ever becomes
    // an authentication boundary.
    value.bytes().fold(0xcbf29ce484222325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
    })
}

fn number(value: Option<&Value>) -> Option<u64> {
    value
        .and_then(Value::as_u64)
        .or_else(|| value.and_then(Value::as_f64).map(|number| number as u64))
}

fn truncate(value: &str, max_bytes: usize) -> String {
    let mut end = 0;
    for character in value.chars() {
        if end + character.len_utf8() > max_bytes {
            break;
        }
        end += character.len_utf8();
    }
    value[..end].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_thread_status_and_preview() {
        let result = json!({
            "data": [{
                "id": "thr-1",
                "name": "Demo",
                "preview": "last message",
                "status": {"type": "active", "activeFlags": ["waitingOnApproval"]},
                "createdAt": 10,
                "updatedAt": 20
            }]
        });
        assert_eq!(parse_sessions(&result)[0].status, "waiting");
        assert_eq!(
            parse_sessions(&result)[0].last_output_summary.as_deref(),
            Some("last message")
        );
    }

    #[test]
    fn parses_multi_bucket_limits_and_credits() {
        let result = json!({
            "rateLimitsByLimitId": {
                "codex": {
                    "limitId": "codex",
                    "primary": {"usedPercent": 56, "windowDurationMins": 300, "resetsAt": 100},
                    "secondary": {"usedPercent": 46, "windowDurationMins": 10080, "resetsAt": 200},
                    "credits": {"hasCredits": false, "unlimited": false, "balance": "0"},
                    "planType": "plus"
                }
            },
            "rateLimitResetCredits": {"availableCount": 1}
        });
        let (buckets, reset) = parse_rate_limits(&result);
        assert_eq!(buckets.len(), 1);
        assert_eq!(
            buckets[0]
                .primary
                .as_ref()
                .and_then(|v| v.window_duration_mins),
            Some(300)
        );
        assert_eq!(
            buckets[0]
                .credits
                .as_ref()
                .and_then(|v| v.balance.as_deref()),
            Some("0")
        );
        assert_eq!(reset, Some(1));
    }

    #[test]
    fn account_key_does_not_expose_email() {
        let account = json!({"type": "chatgpt", "email": "user@example.com"});
        let key = account_group_key(&account).expect("account key");
        assert!(!key.contains("user@example.com"));
    }

    #[test]
    fn parses_bounded_daily_usage_buckets() {
        let result = json!({
            "dailyUsageBuckets": [
                {"startDate": "2026-09-03", "tokens": 30},
                {"startDate": "bad-date", "tokens": 99},
                {"startDate": "2026-09-01", "tokens": 10},
                {"startDate": "2026-09-03", "tokens": 40},
                {"startDate": "2026-09-02", "tokens": -1}
            ]
        });
        assert_eq!(
            parse_daily_usage(&result),
            vec![
                DailyUsageBucket {
                    start_date: "2026-09-01".to_string(),
                    tokens: 10,
                },
                DailyUsageBucket {
                    start_date: "2026-09-03".to_string(),
                    tokens: 30,
                }
            ]
        );
    }
}
