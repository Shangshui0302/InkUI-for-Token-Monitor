# Token Monitor 墨水屏看板

当前方向：复用 Token Monitor 采集与本机留存，提供可在 Poke3 浏览器中打开的完整只读看板。前端入口是 `apps/eink/`，启动入口是 `tools/token-monitor-eink.py`；无需 npm 安装、前端构建或 Tauri。

看板有总览、额度、会话、活动、明细五页，支持日／月／累计切换；展示 tokens、估算 USD cost、分项与速度，以及 models、clients、projects、devices。墨水屏页面固定在可用视口内，不使用滚动；通过页面、时段、筛选和翻页按钮浏览，长值进入分段详情页查看。会话列表可切换全部／运行中，活动页提供全年灰阶热力图、趋势、每日与每月视图。页面只读，不提供会话控制；归档数据来自本机已有记录，不代表跨设备完整历史，也不表示完整复制了对话正文或工具调用详情。默认每 60 秒读取，可改为每 5 分钟或手动；页面隐藏时暂停，各类数据断线时分别保留最近成功缓存。

## 启动

先启动已配置账号的 Token Monitor。此电脑使用 `arch` Distrobox 中的入口：

```bash
distrobox enter arch -- /home/lishangshui/distrobox/arch/.local/bin/token-monitor-host
```

在当前任务 worktree `/home/lishangshui/.codex/worktrees/poke3-token-monitor/codex-monitor-UI` 的仓库根目录启动墨水屏页面服务。令牌文件放在该 worktree 的忽略目录 `.codex-monitor/view-token.secret`。服务可直接从现有 Token Monitor `credentials.json` 读取 `credentials.hub.hostSecret`，不会把凭据复制到仓库或页面：

```bash
python3 tools/token-monitor-eink.py \
  --hub-credentials-file "/home/lishangshui/distrobox/arch/.config/Token Monitor/credentials.json" \
  --session-archive "/home/lishangshui/distrobox/arch/.config/Token Monitor/session-usage-archive.sqlite" \
  --host 0.0.0.0 --port 18788 \
  --view-token-file "$PWD/.codex-monitor/view-token.secret"
```

当前现场通过 `http://10.229.113.165:18788/` 访问；服务默认端口仍为 `8766`，命令行指定 `18788` 是本次运行配置。Poke3 与来源电脑在同一 Wi-Fi 局域网时，在浏览器打开该地址并输入查看令牌。若尚无令牌文件，可安全生成一个随机令牌；以下操作设置仅所有者可读写，并使用 shell 的禁止覆写模式，已有文件会保持原样：

```bash
mkdir -p .codex-monitor
umask 077
(set -C; python3 -c 'import secrets; print(secrets.token_urlsafe(32))' > "$PWD/.codex-monitor/view-token.secret") \
  || echo '令牌文件已存在或无法创建；请保留现有文件并检查路径/权限。'
```

如需查看令牌内容，应在可信终端中读取该私有文件；不要把令牌贴入 URL、文档、聊天或共享日志。服务只开放固定只读路由：`GET /api/stats`、`GET /api/history`，以及配置归档后可用的 `GET /api/session-archive`。Hub 密钥始终留在电脑端。归档参数可选；页面服务以 SQLite 只读方式读取现有 Token Monitor 会话用量归档（含 WAL），仅输出明确允许的展示字段。成功认证后，查看令牌保存在当前标签页的 `sessionStorage`，可在同一标签页刷新后继续使用；关闭标签页后不再保留。页面不会把查看令牌写入 `localStorage`，额度、历史和本机会话归档分别缓存允许展示的字段；刷新失败或清除缓存时各自独立保留或清除。HTTP 仅用于可信局域网。当前防火墙规则已允许现场端口，无需修改 NixOS 配置。

如不使用现有凭据文件，也可以通过 `TOKEN_MONITOR_SECRET` 或 `--hub-secret-file` 提供 Hub 密钥；查看令牌可用 `EINK_VIEW_TOKEN` 或 `--view-token-file` 提供。私有令牌文件建议权限为 `0600`。

## 验证与边界

```bash
python3 -m unittest discover -s tools -p 'test_token_monitor_eink.py' -v
node --test apps/eink/test_model.mjs apps/eink/test_dashboard_model.mjs
node --check apps/eink/app.js
```

实现与接口依据见 [墨水屏方案和验收记录](docs/token-monitor-eink.md)。2026-10-23 已在 Poke3 Chrome 85 横屏真机验证固定视口总览（12 条、两列）、按钮翻页、详情返回原页、刷新保留页码、运行中筛选和滑动不移动内容；另在 Chrome 85 中模拟 10 种尺寸（128×160 至 1920×1080），完成 140 个页面状态和 40 个图表状态检查，未发现溢出。此前真机链路、数据一致性、缓存、断线恢复等记录见方案文档，并不自动证明本轮新交互。长期耗电和物理残影未测；系统启动未配置。本轮未进行 NixOS/Home Manager 修改、系统 rebuild 或 APK 安装。`/healthz` 只表示页面服务在运行，不表示 Hub 或账号采集健康。

## 历史实现

以下目录及后续章节保留早期自建采集器、Tauri 和设备组实现，供追溯。它们不属于当前墨水屏页面的运行链路，旧 M1/M2/M3/M4 计划不再作为新开发任务。

## 旧实现目录（迁移前保留）

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

Nix derivation 使用源码白名单，不会把仓库旁的 `target/`、`apps/ui/node_modules`、`apps/ui/dist`、本地参考图片或其他未跟踪文件复制进构建源。新增构建期文件时必须同步审查该白名单，不能用重新放宽到整个工作区来解决缺失文件。

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

当前只读监控版 Android Debug 包已在 Poke3 真机构建安装并验证统一安装内页面、本地 IPC、创建设备组、本机重命名及普通退到桌面后的前台连接服务保活。Poke3 已与 Linux 来源电脑完成认证配对，并通过固定 TCP 8788 读取到真实账户额度快照；Android 端不运行 Codex，只读查看额度、用量、项目、会话和待审批状态。另一次三节点走查中，Poke3 成功邀请 MyLabWin 的 WSL Arch 临时节点；暂停 MechRevo-NixOS 来源节点约一分钟时，Poke3 与 MyLabWin 仍保持在线。该证据只覆盖 WSL 临时验证，不代表原生 Windows 支持。系统回收恢复、长时间性能及更多页面的数据一致性转为上线后观察项；Poke3 当前安装包落后于仓库最新 `main`，下一次上线只需重新部署并完成一次核心冒烟。最新证据见 `docs/implementation-status.md`，历史 P0 证据见 `docs/p0-android-report.md`。

首次入组、网络要求、来源模式、后台行为与故障恢复见 [docs/operations.md](docs/operations.md)。

首批发布产物、Android 外部签名配置、版本递增规则、覆盖升级的数据保留边界与上线后观察项见 [docs/release-and-upgrade.md](docs/release-and-upgrade.md)。正式发布、上传制品或创建签名密钥仍需单独授权。

`node tools/release-metadata.mjs` 可在不触发构建的情况下检查 Tauri、Cargo 与 Nix 版本一致性；传入制品路径时还会输出大小和 SHA-256 清单。正式 Android 制品使用 `--android-apk` 校验包名、版本与签名，覆盖升级时再用 `--expected-android-cert-sha256` 锁定上一版本证书；首批 Linux 产物使用 `--nix-output` 检查输出布局、许可证和 NAR 元数据。

P0 曾使用会话写入与审批探针验证上游 App Server 能力；当前产品已冻结为只读监控，这些可执行探针不再随工作树保留，避免被误当成当前开发入口。历史结论见 [docs/p0-closeout.md](docs/p0-closeout.md)，完整目标与阶段门禁见 [REWRITE-PLAN.md](REWRITE-PLAN.md)；旧探针和旧实现可从 Git 历史提交 `e09a1a7` 查看。
