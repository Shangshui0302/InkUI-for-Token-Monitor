use std::process::Command;
use std::time::Duration;

use nekodex_appserver_adapter::{
    Adapter, ApprovalDecision, ClientInfo, Incoming, InitializeCapabilities, InitializeParams,
    ThreadStartParams, TurnInterruptParams, TurnStartParams, UserInput,
};
use nekodex_appserver_runtime::{AppServerRuntime, RuntimeConfig, RuntimeEvent};
use serde_json::{json, Value};
use tempfile::TempDir;
use tokio::sync::oneshot;

const MARKER: &str = "nekodex-p0-ui-approval";

pub async fn run_lifecycle() -> Result<Value, String> {
    let repository = temporary_repository()?;
    let cwd = repository
        .path()
        .to_str()
        .ok_or_else(|| "临时仓库路径不是 UTF-8".to_owned())?
        .to_owned();
    let mut runtime = AppServerRuntime::spawn(RuntimeConfig::default())
        .await
        .map_err(|error| error.to_string())?;
    let client = runtime.client();

    client
        .request(|adapter| {
            adapter.initialize(InitializeParams {
                client_info: ClientInfo {
                    name: "nekodex_p0_shell".into(),
                    title: Some("Nekodex P0 Shell".into()),
                    version: env!("CARGO_PKG_VERSION").into(),
                },
                capabilities: Some(InitializeCapabilities {
                    experimental_api: true,
                    request_attestation: false,
                    opt_out_notification_methods: Vec::new(),
                }),
            })
        })
        .await
        .map_err(|error| error.to_string())?;
    client
        .send(
            Adapter::new()
                .initialized()
                .map_err(|error| error.to_string())?,
        )
        .await
        .map_err(|error| error.to_string())?;

    let thread = client
        .request({
            let cwd = cwd.clone();
            move |adapter| {
                adapter.thread_start(ThreadStartParams {
                    cwd: Some(cwd),
                    approval_policy: Some("untrusted".into()),
                    approvals_reviewer: Some("user".into()),
                    sandbox: Some("workspace-write".into()),
                    ephemeral: Some(true),
                    service_name: Some("nekodex_p0_shell".into()),
                })
            }
        })
        .await
        .map_err(|error| error.to_string())?;
    let thread_id = string_at(&thread, &["thread", "id"])?;
    let turn = client
        .request({
            let thread_id = thread_id.clone();
            move |adapter| {
                adapter.turn_start(TurnStartParams {
                    thread_id,
                    input: vec![UserInput::text(format!(
                        "Run exactly this shell command and do nothing else: printf {MARKER} > {MARKER}.txt && sleep 30"
                    ))],
                })
            }
        })
        .await
        .map_err(|error| error.to_string())?;
    let turn_id = string_at(&turn, &["turn", "id"])?;

    let deadline = tokio::time::sleep(Duration::from_secs(90));
    tokio::pin!(deadline);
    let mut approval_seen = false;
    let mut interrupt_accepted = false;
    let mut completed_status = None;
    let mut interrupt_result: Option<oneshot::Receiver<Result<Value, String>>> = None;

    loop {
        if approval_seen && interrupt_accepted && completed_status.is_some() {
            break;
        }
        tokio::select! {
            _ = &mut deadline => return Err("P0 App Server 生命周期验证超时".into()),
            result = async {
                match &mut interrupt_result {
                    Some(receiver) => receiver.await.ok(),
                    None => std::future::pending().await,
                }
            } => {
                interrupt_result = None;
                match result {
                    Some(Ok(_)) => interrupt_accepted = true,
                    Some(Err(error)) => return Err(error),
                    None => return Err("停止请求任务意外结束".into()),
                }
            }
            event = runtime.recv_event() => {
                let Some(event) = event else {
                    return Err("App Server 事件流意外关闭".into());
                };
                match event {
                    RuntimeEvent::Terminated(error) => return Err(error.to_string()),
                    RuntimeEvent::Incoming(Incoming::Approval(request)) => {
                        let matches = match &request {
                            nekodex_appserver_adapter::ApprovalRequest::Command { context, command, .. } => {
                                context.thread_id == thread_id
                                    && context.turn_id == turn_id
                                    && command.as_deref().is_some_and(|value| value.contains(MARKER))
                            }
                            nekodex_appserver_adapter::ApprovalRequest::FileChange { .. } => false,
                        };
                        let response = Adapter::new()
                            .approval_response(
                                &request,
                                if matches { ApprovalDecision::Accept } else { ApprovalDecision::Decline },
                            )
                            .map_err(|error| error.to_string())?;
                        client.send(response).await.map_err(|error| error.to_string())?;
                        if matches && !approval_seen {
                            approval_seen = true;
                            let delayed_client = client.clone();
                            let delayed_thread = thread_id.clone();
                            let delayed_turn = turn_id.clone();
                            let (sender, receiver) = oneshot::channel();
                            tokio::spawn(async move {
                                tokio::time::sleep(Duration::from_millis(1200)).await;
                                let result = delayed_client
                                    .request(move |adapter| {
                                        adapter.turn_interrupt(TurnInterruptParams {
                                            thread_id: delayed_thread,
                                            turn_id: delayed_turn,
                                        })
                                    })
                                    .await
                                    .map_err(|error| error.to_string());
                                let _ = sender.send(result);
                            });
                            interrupt_result = Some(receiver);
                        }
                    }
                    RuntimeEvent::Incoming(Incoming::Notification { method, params })
                        if method == "turn/completed"
                            && params.pointer("/turn/id").and_then(Value::as_str) == Some(turn_id.as_str()) =>
                    {
                        completed_status = params
                            .pointer("/turn/status")
                            .and_then(Value::as_str)
                            .map(str::to_owned);
                    }
                    RuntimeEvent::Incoming(_) => {}
                }
            }
        }
    }

    let status = completed_status.expect("loop requires a completed status");
    let exit = runtime
        .shutdown()
        .await
        .map_err(|error| error.to_string())?;
    Ok(json!({
        "threadId": thread_id,
        "turnId": turn_id,
        "approvalSeen": approval_seen,
        "interruptAccepted": interrupt_accepted,
        "status": status,
        "appServerExitCode": exit.code(),
        "ephemeral": true,
    }))
}

fn temporary_repository() -> Result<TempDir, String> {
    let repository = tempfile::Builder::new()
        .prefix("nekodex-p0-")
        .tempdir()
        .map_err(|error| format!("无法创建临时仓库：{error}"))?;
    let status = Command::new("git")
        .args(["init", "-q"])
        .current_dir(repository.path())
        .status()
        .map_err(|error| format!("无法启动 git init：{error}"))?;
    if !status.success() {
        return Err(format!("git init 失败：{status}"));
    }
    Ok(repository)
}

fn string_at(value: &Value, path: &[&str]) -> Result<String, String> {
    let mut current = value;
    for segment in path {
        current = current
            .get(*segment)
            .ok_or_else(|| format!("App Server 响应缺少字段 {}", path.join(".")))?;
    }
    current
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| format!("App Server 字段 {} 不是字符串", path.join(".")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "runs a real Codex turn and consumes account usage"]
    async fn real_appserver_lifecycle_reaches_interrupted() {
        let result = run_lifecycle().await.unwrap();
        assert_eq!(result["approvalSeen"], true);
        assert_eq!(result["interruptAccepted"], true);
        assert_eq!(result["status"], "interrupted");
        assert_eq!(result["ephemeral"], true);
    }
}
