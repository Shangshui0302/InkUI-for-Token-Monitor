//! Bounded Tokio runtime for a Codex App Server JSONL subprocess.
//!
//! [`RuntimeClient::request`] serializes request construction, stdin writes,
//! and response routing through one actor. Callers should continuously drain
//! [`AppServerRuntime::recv_event`]: its bounded channel intentionally applies
//! backpressure to stdout when unsolicited events arrive faster than they are
//! consumed.

use std::collections::HashMap;
use std::ffi::OsString;
use std::fmt;
use std::process::ExitStatus;
use std::time::Duration;

use nekodex_appserver_adapter::{Adapter, AdapterError, Incoming, Outbound, RequestId};
use serde_json::Value;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, Command};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio::time::{self, Instant};

pub const DEFAULT_MAX_FRAME_BYTES: usize = 512 * 1024;
pub const DEFAULT_CHANNEL_CAPACITY: usize = 64;
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Debug)]
pub struct RuntimeConfig {
    pub program: OsString,
    pub args: Vec<OsString>,
    pub request_timeout: Duration,
    pub command_capacity: usize,
    pub frame_capacity: usize,
    pub event_capacity: usize,
    pub max_frame_bytes: usize,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self::new("codex", ["app-server"])
    }
}

impl RuntimeConfig {
    pub fn new<I, S>(program: impl Into<OsString>, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        Self {
            program: program.into(),
            args: args.into_iter().map(Into::into).collect(),
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
            command_capacity: DEFAULT_CHANNEL_CAPACITY,
            frame_capacity: DEFAULT_CHANNEL_CAPACITY,
            event_capacity: DEFAULT_CHANNEL_CAPACITY,
            max_frame_bytes: DEFAULT_MAX_FRAME_BYTES,
        }
    }

    fn validate(&self) -> Result<(), RuntimeError> {
        if self.command_capacity == 0 || self.frame_capacity == 0 || self.event_capacity == 0 {
            return Err(RuntimeError::InvalidConfig(
                "channel capacities must be greater than zero".into(),
            ));
        }
        if self.max_frame_bytes == 0 {
            return Err(RuntimeError::InvalidConfig(
                "max_frame_bytes must be greater than zero".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum RuntimeError {
    InvalidConfig(String),
    Spawn {
        program: String,
        message: String,
    },
    MissingPipe(&'static str),
    Stdin(String),
    Stdout(String),
    InvalidUtf8(String),
    FrameTooLarge {
        limit: usize,
    },
    Adapter(String),
    RequestHasNoId {
        method: String,
    },
    DuplicateRequestId(RequestId),
    RequestTimeout {
        method: String,
        timeout: Duration,
    },
    ServerResponse {
        method: String,
        code: i64,
        message: String,
        data: Option<Value>,
    },
    ProcessExited {
        code: Option<i32>,
    },
    RuntimeStopped(String),
    QueueClosed,
    TaskJoin(String),
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfig(message) => write!(f, "invalid runtime configuration: {message}"),
            Self::Spawn { program, message } => {
                write!(f, "failed to spawn {program}: {message}")
            }
            Self::MissingPipe(pipe) => write!(f, "spawned process has no piped {pipe}"),
            Self::Stdin(message) => write!(f, "failed to write app-server stdin: {message}"),
            Self::Stdout(message) => write!(f, "failed to read app-server stdout: {message}"),
            Self::InvalidUtf8(message) => write!(f, "app-server frame is not UTF-8: {message}"),
            Self::FrameTooLarge { limit } => {
                write!(f, "app-server frame exceeds the {limit}-byte limit")
            }
            Self::Adapter(message) => write!(f, "app-server protocol error: {message}"),
            Self::RequestHasNoId { method } => write!(f, "request {method} has no id"),
            Self::DuplicateRequestId(id) => write!(f, "duplicate outstanding request id {id:?}"),
            Self::RequestTimeout { method, timeout } => {
                write!(f, "request {method} timed out after {timeout:?}")
            }
            Self::ServerResponse {
                method,
                code,
                message,
                ..
            } => write!(
                f,
                "request {method} failed with server error {code}: {message}"
            ),
            Self::ProcessExited { code } => write!(f, "app-server exited with code {code:?}"),
            Self::RuntimeStopped(message) => write!(f, "app-server runtime stopped: {message}"),
            Self::QueueClosed => f.write_str("app-server runtime command queue is closed"),
            Self::TaskJoin(message) => write!(f, "app-server runtime task failed: {message}"),
        }
    }
}

impl std::error::Error for RuntimeError {}

impl From<AdapterError> for RuntimeError {
    fn from(value: AdapterError) -> Self {
        Self::Adapter(value.to_string())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum RuntimeEvent {
    Incoming(Incoming),
    Terminated(RuntimeError),
}

type BuildRequest =
    Box<dyn FnOnce(&mut Adapter) -> Result<Outbound, AdapterError> + Send + 'static>;

enum ActorCommand {
    Request {
        build: BuildRequest,
        timeout: Duration,
        reply: oneshot::Sender<Result<Value, RuntimeError>>,
    },
    Send {
        outbound: Outbound,
        reply: oneshot::Sender<Result<(), RuntimeError>>,
    },
    Shutdown {
        reply: oneshot::Sender<Result<(), RuntimeError>>,
    },
}

struct Pending {
    method: String,
    timeout: Duration,
    deadline: Instant,
    reply: oneshot::Sender<Result<Value, RuntimeError>>,
}

enum ReaderMessage {
    Frame(String),
    Failed(RuntimeError),
    Eof,
}

#[derive(Clone)]
pub struct RuntimeClient {
    commands: mpsc::Sender<ActorCommand>,
    default_timeout: Duration,
}

impl RuntimeClient {
    pub async fn request<F>(&self, build: F) -> Result<Value, RuntimeError>
    where
        F: FnOnce(&mut Adapter) -> Result<Outbound, AdapterError> + Send + 'static,
    {
        self.request_with_timeout(self.default_timeout, build).await
    }

    pub async fn request_with_timeout<F>(
        &self,
        timeout: Duration,
        build: F,
    ) -> Result<Value, RuntimeError>
    where
        F: FnOnce(&mut Adapter) -> Result<Outbound, AdapterError> + Send + 'static,
    {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(ActorCommand::Request {
                build: Box::new(build),
                timeout,
                reply,
            })
            .await
            .map_err(|_| RuntimeError::QueueClosed)?;
        response.await.map_err(|_| RuntimeError::QueueClosed)?
    }

    pub async fn send(&self, outbound: Outbound) -> Result<(), RuntimeError> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(ActorCommand::Send { outbound, reply })
            .await
            .map_err(|_| RuntimeError::QueueClosed)?;
        response.await.map_err(|_| RuntimeError::QueueClosed)?
    }
}

pub struct AppServerRuntime {
    client: RuntimeClient,
    events: mpsc::Receiver<RuntimeEvent>,
    task: JoinHandle<Result<ExitStatus, RuntimeError>>,
}

impl AppServerRuntime {
    pub async fn spawn(config: RuntimeConfig) -> Result<Self, RuntimeError> {
        config.validate()?;

        let mut command = Command::new(&config.program);
        command
            .args(&config.args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        let mut child = command.spawn().map_err(|error| RuntimeError::Spawn {
            program: config.program.to_string_lossy().into_owned(),
            message: error.to_string(),
        })?;
        let stdin = child
            .stdin
            .take()
            .ok_or(RuntimeError::MissingPipe("stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or(RuntimeError::MissingPipe("stdout"))?;
        let mut stderr = child
            .stderr
            .take()
            .ok_or(RuntimeError::MissingPipe("stderr"))?;

        let (command_tx, command_rx) = mpsc::channel(config.command_capacity);
        let (frame_tx, frame_rx) = mpsc::channel(config.frame_capacity);
        let (event_tx, event_rx) = mpsc::channel(config.event_capacity);

        tokio::spawn(async move {
            let _ = tokio::io::copy(&mut stderr, &mut tokio::io::sink()).await;
        });
        tokio::spawn(read_stdout(
            BufReader::new(stdout),
            frame_tx,
            config.max_frame_bytes,
        ));

        let default_timeout = config.request_timeout;
        let task = tokio::spawn(run_actor(
            child,
            stdin,
            command_rx,
            frame_rx,
            event_tx,
            config.max_frame_bytes,
        ));

        Ok(Self {
            client: RuntimeClient {
                commands: command_tx,
                default_timeout,
            },
            events: event_rx,
            task,
        })
    }

    pub fn client(&self) -> RuntimeClient {
        self.client.clone()
    }

    pub async fn recv_event(&mut self) -> Option<RuntimeEvent> {
        self.events.recv().await
    }

    pub async fn shutdown(self) -> Result<ExitStatus, RuntimeError> {
        let (reply, response) = oneshot::channel();
        self.client
            .commands
            .send(ActorCommand::Shutdown { reply })
            .await
            .map_err(|_| RuntimeError::QueueClosed)?;
        response.await.map_err(|_| RuntimeError::QueueClosed)??;
        self.task
            .await
            .map_err(|error| RuntimeError::TaskJoin(error.to_string()))?
    }

    pub async fn wait(self) -> Result<ExitStatus, RuntimeError> {
        self.task
            .await
            .map_err(|error| RuntimeError::TaskJoin(error.to_string()))?
    }
}

async fn write_outbound(
    stdin: &mut ChildStdin,
    outbound: &Outbound,
    max_frame_bytes: usize,
) -> Result<(), RuntimeError> {
    if outbound.line.len() > max_frame_bytes {
        return Err(RuntimeError::FrameTooLarge {
            limit: max_frame_bytes,
        });
    }
    stdin
        .write_all(outbound.line.as_bytes())
        .await
        .map_err(|error| RuntimeError::Stdin(error.to_string()))?;
    stdin
        .flush()
        .await
        .map_err(|error| RuntimeError::Stdin(error.to_string()))
}

async fn read_stdout<R>(mut reader: R, frames: mpsc::Sender<ReaderMessage>, max_frame_bytes: usize)
where
    R: AsyncBufRead + Unpin,
{
    loop {
        match read_bounded_line(&mut reader, max_frame_bytes).await {
            Ok(Some(bytes)) => match String::from_utf8(bytes) {
                Ok(frame) => {
                    if frames.send(ReaderMessage::Frame(frame)).await.is_err() {
                        return;
                    }
                }
                Err(error) => {
                    let _ = frames
                        .send(ReaderMessage::Failed(RuntimeError::InvalidUtf8(
                            error.to_string(),
                        )))
                        .await;
                    return;
                }
            },
            Ok(None) => {
                let _ = frames.send(ReaderMessage::Eof).await;
                return;
            }
            Err(error) => {
                let _ = frames.send(ReaderMessage::Failed(error)).await;
                return;
            }
        }
    }
}

async fn read_bounded_line<R>(
    reader: &mut R,
    max_frame_bytes: usize,
) -> Result<Option<Vec<u8>>, RuntimeError>
where
    R: AsyncBufRead + Unpin,
{
    let mut line = Vec::with_capacity(max_frame_bytes.min(8192));
    loop {
        let available = reader
            .fill_buf()
            .await
            .map_err(|error| RuntimeError::Stdout(error.to_string()))?;
        if available.is_empty() {
            return if line.is_empty() {
                Ok(None)
            } else {
                Ok(Some(line))
            };
        }
        let take = available
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(available.len(), |position| position + 1);
        if line.len().saturating_add(take) > max_frame_bytes {
            return Err(RuntimeError::FrameTooLarge {
                limit: max_frame_bytes,
            });
        }
        line.extend_from_slice(&available[..take]);
        let ended = available[take - 1] == b'\n';
        reader.consume(take);
        if ended {
            return Ok(Some(line));
        }
    }
}

async fn run_actor(
    mut child: tokio::process::Child,
    mut stdin: ChildStdin,
    mut commands: mpsc::Receiver<ActorCommand>,
    mut frames: mpsc::Receiver<ReaderMessage>,
    events: mpsc::Sender<RuntimeEvent>,
    max_frame_bytes: usize,
) -> Result<ExitStatus, RuntimeError> {
    let mut adapter = Adapter::new();
    let mut pending: HashMap<RequestId, Pending> = HashMap::new();
    let mut stdout_open = true;

    loop {
        let deadline = pending
            .values()
            .map(|request| request.deadline)
            .min()
            .unwrap_or_else(|| Instant::now() + Duration::from_secs(365 * 24 * 60 * 60));

        tokio::select! {
            status = child.wait() => {
                let status = match status {
                    Ok(status) => status,
                    Err(error) => {
                        let error = RuntimeError::RuntimeStopped(error.to_string());
                        fail_all(&mut pending, error.clone());
                        return Err(error);
                    }
                };
                let error = RuntimeError::ProcessExited { code: status.code() };
                fail_all(&mut pending, error.clone());
                let _ = events.send(RuntimeEvent::Terminated(error)).await;
                return Ok(status);
            }
            message = frames.recv(), if stdout_open => {
                match message {
                    Some(ReaderMessage::Frame(frame)) => {
                        match adapter.receive_line(&frame) {
                            Ok(Incoming::Response { id, result, .. }) => {
                                if let Some(request) = pending.remove(&id) {
                                    let _ = request.reply.send(Ok(result));
                                } else {
                                    if events.send(RuntimeEvent::Incoming(Incoming::UnknownResponse {
                                        id,
                                        result: Some(result),
                                        error: None,
                                    })).await.is_err() {
                                        let error = RuntimeError::RuntimeStopped("event receiver closed".into());
                                        return terminate_with_error(&mut child, &mut pending, &events, error).await;
                                    }
                                }
                            }
                            Ok(Incoming::ErrorResponse { id, method, error }) => {
                                if let Some(request) = pending.remove(&id) {
                                    let _ = request.reply.send(Err(RuntimeError::ServerResponse {
                                        method,
                                        code: error.code,
                                        message: error.message,
                                        data: error.data,
                                    }));
                                } else {
                                    if events.send(RuntimeEvent::Incoming(Incoming::UnknownResponse {
                                        id,
                                        result: None,
                                        error: Some(error),
                                    })).await.is_err() {
                                        let error = RuntimeError::RuntimeStopped("event receiver closed".into());
                                        return terminate_with_error(&mut child, &mut pending, &events, error).await;
                                    }
                                }
                            }
                            Ok(incoming) => {
                                if events.send(RuntimeEvent::Incoming(incoming)).await.is_err() {
                                    let error = RuntimeError::RuntimeStopped("event receiver closed".into());
                                    return terminate_with_error(&mut child, &mut pending, &events, error).await;
                                }
                            }
                            Err(error) => {
                                return terminate_with_error(&mut child, &mut pending, &events, error.into()).await;
                            }
                        }
                    }
                    Some(ReaderMessage::Failed(error)) => {
                        return terminate_with_error(&mut child, &mut pending, &events, error).await;
                    }
                    Some(ReaderMessage::Eof) | None => stdout_open = false,
                }
            }
            command = commands.recv() => {
                match command {
                    Some(ActorCommand::Request { build, timeout, reply }) => {
                        let outbound = match build(&mut adapter) {
                            Ok(outbound) => outbound,
                            Err(error) => {
                                let _ = reply.send(Err(error.into()));
                                continue;
                            }
                        };
                        let Some(id) = outbound.id.clone() else {
                            let _ = reply.send(Err(RuntimeError::RequestHasNoId {
                                method: outbound.method,
                            }));
                            continue;
                        };
                        if pending.contains_key(&id) {
                            let _ = reply.send(Err(RuntimeError::DuplicateRequestId(id)));
                            continue;
                        }
                        if let Err(error) = write_outbound(&mut stdin, &outbound, max_frame_bytes).await {
                            let _ = reply.send(Err(error.clone()));
                            return terminate_with_error(&mut child, &mut pending, &events, error).await;
                        }
                        pending.insert(id, Pending {
                            method: outbound.method,
                            timeout,
                            deadline: Instant::now() + timeout,
                            reply,
                        });
                    }
                    Some(ActorCommand::Send { outbound, reply }) => {
                        let result = write_outbound(&mut stdin, &outbound, max_frame_bytes).await;
                        let failed = result.as_ref().err().cloned();
                        let _ = reply.send(result);
                        if let Some(error) = failed {
                            return terminate_with_error(&mut child, &mut pending, &events, error).await;
                        }
                    }
                    Some(ActorCommand::Shutdown { reply }) => {
                        let result = child.start_kill().map_err(|error| RuntimeError::RuntimeStopped(error.to_string()));
                        let failed = result.as_ref().err().cloned();
                        let _ = reply.send(result);
                        if let Some(error) = failed {
                            fail_all(&mut pending, error.clone());
                            return Err(error);
                        }
                    }
                    None => {
                        let error = RuntimeError::RuntimeStopped("all clients were dropped".into());
                        return terminate_with_error(&mut child, &mut pending, &events, error).await;
                    }
                }
            }
            _ = time::sleep_until(deadline), if !pending.is_empty() => {
                let now = Instant::now();
                let timed_out = pending.values().find(|request| request.deadline <= now);
                if let Some(request) = timed_out {
                    let error = RuntimeError::RequestTimeout {
                        method: request.method.clone(),
                        timeout: request.timeout,
                    };
                    return terminate_with_error(&mut child, &mut pending, &events, error).await;
                }
            }
        }
    }
}

fn fail_all(pending: &mut HashMap<RequestId, Pending>, error: RuntimeError) {
    for (_, request) in pending.drain() {
        let _ = request.reply.send(Err(error.clone()));
    }
}

async fn terminate_with_error(
    child: &mut tokio::process::Child,
    pending: &mut HashMap<RequestId, Pending>,
    events: &mpsc::Sender<RuntimeEvent>,
    error: RuntimeError,
) -> Result<ExitStatus, RuntimeError> {
    fail_all(pending, error.clone());
    let _ = child.start_kill();
    let _ = events.try_send(RuntimeEvent::Terminated(error.clone()));
    match child.wait().await {
        Ok(_) => Err(error),
        Err(wait_error) => Err(RuntimeError::RuntimeStopped(format!(
            "{error}; also failed to wait for process: {wait_error}"
        ))),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use nekodex_appserver_adapter::{ClientInfo, InitializeCapabilities, InitializeParams};
    use serde_json::json;

    fn shell(script: &str) -> RuntimeConfig {
        let mut config = RuntimeConfig::new("sh", ["-c", script]);
        config.request_timeout = Duration::from_millis(300);
        config
    }

    fn initialize(adapter: &mut Adapter) -> Result<Outbound, AdapterError> {
        adapter.initialize(InitializeParams {
            client_info: ClientInfo {
                name: "runtime-test".into(),
                title: None,
                version: "0.1.0".into(),
            },
            capabilities: Some(InitializeCapabilities::default()),
        })
    }

    #[tokio::test]
    async fn correlates_request_and_response_through_child_process() {
        let runtime = AppServerRuntime::spawn(shell(
            r#"IFS= read -r line; id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p'); printf '{"id":%s,"result":{"ready":true}}\n' "$id"; sleep 60"#,
        ))
        .await
        .unwrap();
        let result = runtime.client().request(initialize).await.unwrap();
        assert_eq!(result, json!({"ready": true}));
        runtime.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn rejects_oversized_stdout_frame() {
        let mut config = shell("printf '123456789'");
        config.max_frame_bytes = 8;
        let mut runtime = AppServerRuntime::spawn(config).await.unwrap();
        assert_eq!(
            runtime.recv_event().await,
            Some(RuntimeEvent::Terminated(RuntimeError::FrameTooLarge {
                limit: 8
            }))
        );
        assert!(matches!(
            runtime.wait().await,
            Err(RuntimeError::FrameTooLarge { limit: 8 })
        ));
    }

    #[tokio::test]
    async fn rejects_oversized_stdin_frame() {
        let mut config = shell("sleep 60");
        config.max_frame_bytes = 8;
        let runtime = AppServerRuntime::spawn(config).await.unwrap();
        let error = runtime
            .client()
            .send(Outbound {
                id: None,
                method: "test".into(),
                line: "123456789".into(),
            })
            .await
            .unwrap_err();
        assert_eq!(error, RuntimeError::FrameTooLarge { limit: 8 });
        assert!(matches!(
            runtime.wait().await,
            Err(RuntimeError::FrameTooLarge { limit: 8 })
        ));
    }

    #[tokio::test]
    async fn times_out_pending_request_and_stops_process() {
        let runtime = AppServerRuntime::spawn(shell("IFS= read -r line; sleep 60"))
            .await
            .unwrap();
        let error = runtime.client().request(initialize).await.unwrap_err();
        assert!(matches!(
            error,
            RuntimeError::RequestTimeout { ref method, .. } if method == "initialize"
        ));
        assert!(matches!(
            runtime.wait().await,
            Err(RuntimeError::RequestTimeout { .. })
        ));
    }

    #[tokio::test]
    async fn process_exit_fails_pending_request_explicitly() {
        let runtime = AppServerRuntime::spawn(shell("IFS= read -r line; exit 7"))
            .await
            .unwrap();
        let error = runtime.client().request(initialize).await.unwrap_err();
        assert_eq!(error, RuntimeError::ProcessExited { code: Some(7) });
        let status = runtime.wait().await.unwrap();
        assert_eq!(status.code(), Some(7));
    }
}
