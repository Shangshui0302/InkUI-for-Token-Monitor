# P0 能力验证矩阵

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

应用壳现已通过 `p0_appserver_lifecycle` Tauri IPC 暴露同类验证。该路径使用 Rust 适配器和受控子进程运行时，带有有界队列、请求超时、512 KiB 帧上限和退出时未决请求失败语义；临时仓库随验证结束删除。被忽略的真实集成测试需显式运行，已在本机执行并通过，最终状态为 `interrupted`。Poke3 调用该命令只返回“控制端不在本机运行 App Server”；正式跨设备调用留待网络层。当前尚未在已打包桌面窗口中人工点击该按钮，因此代码路径与真实 App Server 已验证，窗口级点击仍是待验收项。

持久化探针由第一个 App Server 进程创建并完成一个持久会话，关闭该进程后由第二个进程通过 `thread/resume` 恢复并通过 `thread/read` 读取，随后删除探针会话。该证据只覆盖空闲历史会话；不同 App Server 进程或桌面客户端持有运行中轮次时的接管、停止和审批路由仍是明确限制。

文件审批探针在隔离临时 Git 仓库中要求 App Server 仅通过文件变更工具创建唯一 marker。探针先关联同一 thread/turn 的 fileChange item，再仅批准 item ID 对应且变更路径精确等于 marker 的 `item/fileChange/requestApproval`；所有命令与其他审批均拒绝。一次真实样本中，文件审批 1 次、命令审批 0 次，resolved、fileChange completed、turn completed、marker 精确内容和唯一工作树变更均通过；ephemeral 会话与临时仓库已清理。该结果证明锁定版本当前可用，不保证其他版本或模型每次都选择文件变更工具。
