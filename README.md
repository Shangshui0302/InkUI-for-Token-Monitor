# Codex 跨设备监控台

局域网内的个人 Codex 设备组。设备一次入组后共享成员资格，额度、活动、会话和待审批状态直接由对应电脑提供；PC 与 Android 使用同一套前端。

当前产品边界为只读监控：不提供远程创建、发送消息、停止任务或处理审批。设备组三节点、跨设备数据一致性和长期性能仍待集成验收：

电脑来源优先连接已经运行的官方本地共享 App Server daemon，从而读取该 daemon 已加载会话的实时状态；daemon 不可用时回退到独立只读 App Server。回退模式仍可读取额度和持久化历史，但界面不会把 `notLoaded` 或不可见的审批误报成实时状态。用户可以在本机界面明确调用官方共享 daemon 命令；应用不会在后台静默修改它。该命令当前要求 Codex 安装器管理的 standalone CLI，Nix 打包的 CLI 会保留历史回退来源并显示官方失败原因。

- `apps/ui`：React + TypeScript 统一响应式界面，经 Nix 固定依赖并随应用安装，无远程网页依赖；
- `apps/shell`：Tauri 2 桌面/Android 壳，Android 最低 API 29；
- `crates/device-core`：设备身份、签名名册、PAKE 入组、Noise 直连、mDNS、重连与 SQLite 缓存；
- `crates/source-service`：项目白名单、App Server 生命周期与只读监控数据；
- `crates/appserver-adapter`：App Server JSONL 请求关联、额度/用量、事件与审批路由；
- `crates/appserver-runtime`：有界队列、帧上限、超时及退出语义明确的子进程运行时；
- `crates/domain`：签名成员 DAG、SPAKE2 配对原语与本地防回滚检查点；
- `crates/protocol`：跨设备领域对象、只读能力、游标和进程世代契约；
- `generated/codex-app-server`：由当前 Codex CLI 生成的协议类型；
- `docs`：成员控制面 ADR、能力矩阵和完整重写方案。

桌面端设计为关闭主窗口后继续在后台维持设备组和来源任务；通过系统托盘重新打开，或从托盘菜单显式退出。Android 仍由前台服务承担后台可见性与恢复调度，不使用桌面托盘逻辑。当前平台运行证据以 `docs/implementation-status.md` 为准。

日常开发在已配置国内 Cargo/npm 镜像的 `arch` Distrobox 中进行。项目文件留在宿主 worktree，`distrobox enter` 会保留当前目录；编译缓存只写入容器专用 home，不再让宿主 Nix 构建产生大体积 store 闭包：

```bash
distrobox enter arch
export CARGO_TARGET_DIR="$HOME/.cache/nekodex/target"
export CARGO_BUILD_JOBS=1
npm run build --prefix apps/ui
cargo check -p nekodex-shell --lib -j1
```

只有相关输入变化或交付需要时才运行更大范围的检查。桌面 release 可在同一容器中执行 `cargo tauri build --no-bundle --config apps/shell/src-tauri/tauri.conf.json`；正式去掉 `--no-bundle` 时才生成目标平台安装包。Android 的 `gen/android/tauri.settings.gradle` 是 Tauri CLI 按当前机器依赖路径生成的临时文件，已被该目录的 `.gitignore` 排除，不应提交或手工固化其中的绝对路径。

统一前端构建入口是 `./tools/build-frontend.sh`（或在 `apps/ui` 中运行 `npm run build`）；生成目录不会纳入 Git。构建脚本不会在依赖缺失时隐式调用 Nix，避免日常开发意外创建新的 store 闭包；缺少依赖会直接提示先在当前开发环境安装锁文件依赖。Nix 打包通过显式的 `NEKODEX_FRONTEND_PREBUILT=1` 使用 derivation 已提供的前端产物。

首次在 Windows 或其他没有 Nix 的环境中构建时，在仓库根目录执行一次 `npm install --prefix apps/ui`，它会根据 `apps/ui/package.json` 安装 React、react-dom、scheduler 和 esbuild 到 `apps/ui/node_modules`。之后运行 `npm run build --prefix apps/ui`。依赖安装和构建是两个独立步骤；不要把手工准备的 `node_modules` 当作构建完成的证明。

已有且最新的 `apps/ui/dist` 会直接复用，不会重复构建。Windows 构建要求先完成上述依赖安装。

`flake.nix` 只保留可复现的 Linux 打包定义，不再暴露宿主开发或 Android devShell，避免误执行 `nix develop` 拉入完整 SDK/NDK 闭包。只有明确需要验证 Linux Nix 包时才由用户手动执行：

```bash
nix build path:.#nekodex
nix run path:.
```

这两条命令使用锁定的 Nix 输入构建应用及其运行时依赖；`nix build` 不会把应用安装进系统，也不会触发 NixOS rebuild。自动化开发流程不得调用它们，也不得自动执行 `nixos-rebuild`。

Android 同样在 `arch` 容器中使用已准备的 SDK/NDK/JDK，并限制 Cargo 与 Gradle 并发：

```bash
export ANDROID_HOME="$HOME/Android/Sdk"
export ANDROID_SDK_ROOT="$ANDROID_HOME"
export NDK_HOME="$ANDROID_HOME/ndk/28.0.13004108"
export JAVA_HOME=/usr/lib/jvm/java-17-openjdk
export CARGO_TARGET_DIR="$HOME/.cache/nekodex/target"
export CARGO_BUILD_JOBS=1
export GRADLE_OPTS="${GRADLE_OPTS:+$GRADLE_OPTS }-Dorg.gradle.workers.max=1"
cargo tauri android build --debug --target aarch64
```

当前只读监控版 Android Debug 包已在 Poke3 真机构建安装并验证统一安装内页面、本地 IPC、创建设备组、本机重命名及普通退到桌面后的前台连接服务保活。Poke3 已与 Linux 来源电脑完成认证配对，并通过固定 TCP 8788 读取到真实账户额度快照；Android 端不运行 Codex，只读查看额度、用量、项目、会话和待审批状态。系统回收恢复、长时间性能及更多页面的数据一致性仍待验收。最新证据见 `docs/implementation-status.md`，历史 P0 证据见 `docs/p0-android-report.md`。

首批发布产物、Android 外部签名配置、版本递增规则、覆盖升级的数据保留边界与尚待验收项目见 [docs/release-and-upgrade.md](docs/release-and-upgrade.md)。正式发布、上传制品或创建签名密钥仍需单独授权。

P0 曾使用会话写入与审批探针验证上游 App Server 能力；当前产品已冻结为只读监控，这些可执行探针不再随工作树保留，避免被误当成当前开发入口。历史结论见 [docs/p0-closeout.md](docs/p0-closeout.md)，完整目标与阶段门禁见 [REWRITE-PLAN.md](REWRITE-PLAN.md)；旧探针和旧实现可从 Git 历史提交 `e09a1a7` 查看。
