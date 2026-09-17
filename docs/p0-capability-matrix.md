# P0 能力验证矩阵

> 历史记录：本矩阵只保存 2026-09-15 的上游能力实验结果。它不表示当前应用提供这些操作。2026-09-16 起产品为只读监控，应用壳中的 `p0_appserver_lifecycle` 命令及全部远程控制 UI 已移除。

| 能力 | 上游接口 | P0 状态 | 验收证据 |
| --- | --- | --- | --- |
| 创建任务 | `thread/start` + `turn/start` | 桌面应用壳真实链路已通过 | Rust IPC 实现返回线程与轮次 ID并收到完成事件；局域网链路待 P3 |
| 发送消息 | `turn/start` | 桌面应用壳真实链路已通过 | Tauri P0 命令创建真实轮次；连续轮次与正式会话 UI 待实现 |
| 运行中追加指导 | `turn/steer` | 本机真实链路已通过 | `expectedTurnId` 匹配并返回同一轮次 ID；过期 ID 测试待补 |
| 停止任务 | `turn/interrupt` | 桌面应用壳真实链路已通过 | 请求成功且最终轮次状态为 `interrupted` |
| 命令审批 | `item/commandExecution/requestApproval` | 桌面应用壳真实链路已通过 | 只接受线程、轮次与固定测试标记均匹配的命令，其余一律拒绝 |
| 文件审批 | `item/fileChange/requestApproval` | 本机真实链路已通过 | 精确匹配唯一 marker 的文件审批后，收到 resolved、fileChange completed 与 turn completed；命令审批为 0 |
| 外部客户端会话控制 | `thread/read/resume` + 轮次接口 | 跨进程读取/恢复空闲持久会话已通过 | 桌面客户端运行中与等待审批状态仍未确认，不宣称可控制 |

协议类型由当前安装的 `codex app-server generate-ts --experimental` 生成并提交。每次升级 Codex 后重新生成，并以差异审查决定适配器版本。

当前基线为 `codex-cli 0.154.0`。生命周期探针在隔离临时 Git 仓库中创建临时线程，收到真实命令审批，接受后对运行中轮次追加指导并中断；结果为 `approvalSeen=true`、`steerAccepted=true`、`interruptAccepted=true`、`status=interrupted`。原始事件日志包含本地路径，不提交仓库。

当时的应用壳曾通过 `p0_appserver_lifecycle` Tauri IPC 暴露同类验证。该路径使用 Rust 适配器和受控子进程运行时，带有有界队列、请求超时、512 KiB 帧上限和退出语义；临时仓库随验证结束删除。该命令现已从应用移除，历史结果只用于解释为何仍保留 App Server 读取适配层。

持久化探针由第一个 App Server 进程创建并完成一个持久会话，关闭该进程后由第二个进程通过 `thread/resume` 恢复并通过 `thread/read` 读取，随后删除探针会话。该证据只覆盖空闲历史会话；不同 App Server 进程或桌面客户端持有运行中轮次时的接管、停止和审批路由仍是明确限制。

文件审批探针在隔离临时 Git 仓库中要求 App Server 仅通过文件变更工具创建唯一 marker。探针先关联同一 thread/turn 的 fileChange item，再仅批准 item ID 对应且变更路径精确等于 marker 的 `item/fileChange/requestApproval`；所有命令与其他审批均拒绝。一次真实样本中，文件审批 1 次、命令审批 0 次，resolved、fileChange completed、turn completed、marker 精确内容和唯一工作树变更均通过；ephemeral 会话与临时仓库已清理。该结果证明锁定版本当前可用，不保证其他版本或模型每次都选择文件变更工具。
