# Codex 跨设备工作台

局域网内的个人 Codex 设备组。设备一次入组后共享成员资格，额度、会话和控制请求直接由对应电脑提供；PC 与 Android 使用同一套前端。

当前处于 P0 可行性验证：

- `apps/ui`：随应用安装的统一响应式界面，无远程网页依赖；
- `apps/shell`：Tauri 2 桌面/Android 壳，Android 最低 API 29；
- `crates/appserver-adapter`：App Server JSONL 请求关联、事件与审批路由；
- `crates/appserver-runtime`：有界队列、帧上限、超时及退出语义明确的子进程运行时；
- `crates/domain`：签名成员 DAG、SPAKE2 配对原语与本地防回滚检查点；
- `crates/protocol`：跨设备领域对象、幂等请求、游标和进程世代契约；
- `generated/codex-app-server`：由当前 Codex CLI 生成的协议类型；
- `docs`：成员控制面 ADR、能力矩阵和完整重写方案。

进入开发环境并运行检查：

```bash
nix develop
cargo test --workspace
cargo tauri build --no-bundle --config apps/shell/src-tauri/tauri.conf.json
```

Android 需使用包含固定 SDK/NDK/JDK 的开发环境：

```bash
nix develop .#android
cargo tauri android build --debug --target aarch64
```

Poke3 真机已验证同一静态页面、本地 IPC、导航、ASCII 输入、演示审批弹层和基础 WebSocket。真实业务数据仍未接入 UI；当前页面只能称 P0 交互探针，不能称完整工作台。验收边界见 `docs/p0-android-report.md` 与 `docs/p0-gap-audit.md`。

App Server 生命周期探针只在隔离临时仓库运行，并可能消耗少量 Codex 用量：

```bash
probe_dir="$(mktemp -d)"
git -C "$probe_dir" init -q
node tools/appserver-lifecycle-probe.mjs "$probe_dir"
```

P0 结论与证据见 [docs/p0-closeout.md](docs/p0-closeout.md)，完整目标与阶段门禁见 [REWRITE-PLAN.md](REWRITE-PLAN.md)。旧实现可从 Git 历史提交 `e09a1a7` 查看。
