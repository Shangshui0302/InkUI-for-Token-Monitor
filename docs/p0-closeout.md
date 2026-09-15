# P0 可行性收尾报告

日期：2026-09-15

结论：通过，允许进入 P1 局域网设备组；以下限制已明确记录，不视为完整产品能力。

## 门禁结果

| P0 门禁 | 结果 | 关键证据 |
| --- | --- | --- |
| PC/Poke3 同一页面 | 通过 | 桌面 release 窗口实际绘制 `apps/ui` 并显示 `linux · 本地资源 · IPC 正常`；Poke3 Android 10/API29、WebView 108 上同一资源及 IPC 正常 |
| 创建→发送→审批→停止 | 通过 | Tauri 壳真实 App Server 集成测试创建临时任务、发送提示、匹配命令审批并停止，最终 `interrupted`；文件审批独立真实探针收到 1 次 fileChange approval、0 次 command approval，并完成 resolved/文件/轮次核验 |
| 外部会话限制 | 通过（限制记录） | 跨 App Server 进程可读取和恢复空闲持久会话；桌面客户端持有的运行中轮次与审批不宣称可接管 |
| 成员撤销与并发模型 | 通过（领域层） | Ed25519 签名成员 DAG、撤销优先、乱序/冲突、三份独立日志、请求幂等、进程世代、旧审批失效和本地防回滚检查点均有确定性测试 |

## 可重复检查

```bash
node --check apps/ui/app.js
node --check tools/appserver-file-approval-probe.mjs
nix develop -c cargo fmt --all -- --check
nix develop -c cargo test --workspace
nix develop -c cargo clippy --workspace --all-targets -- -D warnings
nix develop -c cargo tauri build --no-bundle --config apps/shell/src-tauri/tauri.conf.json
nix develop .#android -c cargo tauri android build --debug --target aarch64
git diff --check
```

真实 App Server 检查会消耗少量账户用量，因此默认测试将应用壳真实测试标为 ignored，需显式运行。文件审批探针同样只在需要重新验证锁定版本时执行。

## 已知边界

- 当前 UI 是共享静态 P0 探针，不是 React/TypeScript 完整工作台；额度、活动、节点和会话内容仍是固定演示数据。
- Poke3 不在本机运行 Codex。正式远程操作依赖 P1/P3 的局域网连接和业务服务。
- 签名、邀请、防回滚与三节点结论目前是领域层证明；密钥安全存储、成员日志持久化、网络握手和连接门禁属于 P1。
- 文件审批成功是当前 `codex-cli 0.154.0` 的一次真实样本；升级后必须重新生成协议并复测。
- 真机仍需在 P4 验证中文输入、长文本、旋转、后台 30 分钟、断网缓存、首屏时间、长期 CPU/PSS、残影和耗电。
- 桌面 release 窗口已在隔离 X11 环境绘制；当前任务环境没有可用窗口输入注入工具，因此未把后端自动测试描述为人工按钮点击。

## 下一阶段

P1 首先实现设备身份持久化、创建/加入组、真实 PAKE 握手、签名名册交换、mDNS 与手工地址、加密 WebSocket、重连和撤销连接门禁；再用 NixOS、第二台电脑和 Poke3 完成真实三节点验收。
