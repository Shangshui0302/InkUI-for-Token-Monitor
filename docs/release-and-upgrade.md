# Nekodex 发布与升级方案

> **发布基线已变更：** 首版实现必须遵循 [简化监控架构](simplified-monitor-architecture.md)。旧 App Server 来源链路不得作为发布候选继续修补或验收；先完成本地文件只读采集器和 Poke3 单来源读取，再评估轻量设备目录。

本文记录首批平台的可交付边界。执行正式发布、上传商店、推送制品或生成正式签名密钥仍需单独授权。

## 当前发布状态（2026-09-20）

M1 是已验证的源码级采集器和浏览器页面，不是已经切换完成的 Tauri/Android 发布物。当前仓库因此还不是新架构的发布候选：M2 需要完成统一前端接入，M3 需要完成 Poke3 通过已知局域网地址读取 M1，之后才按本文件的制品门禁评估首发。旧 8788 设备组/App Server 的安装或运行证据不替代这些门禁。

M1 的验证证据包括标准库编译检查、2 项采集器单元测试、真实宿主任务/额度/活动/会话读取，以及非回环令牌拒绝和认证请求检查；完整记录见 [实现状态](implementation-status.md)。

## 最小首发门禁

首发目标是尽快提供 NixOS/Linux 与 Poke3 可用的只读 MVP。发布候选只要求：本机采集器不启动 App Server、不访问 OpenAI 在线接口即可读取真实任务、额度、活动和会话摘要；PC 与 Poke3 两个目标产物成功生成并启动；Poke3 能通过已知局域网来源地址读取一个 Linux 来源；远程写操作不存在；版本、包名、签名和制品哈希可记录。每个平台只做一次核心冒烟，不重复运行已经由相同输入证明通过的全量测试。

全页面语义矩阵、长时间待机、系统回收、慢消费者、极端网络、不同邀请顺序、在线撤销收敛、残影、长期耗电、Windows/macOS 和其他安装包格式均不阻塞首发。没有具体缺陷或威胁证据时，不新增测试层、兼容层或安全硬化；身份认证、密钥保护、只读边界和签名连续性仍不可跳过。

## 首批产物

- NixOS/Linux：首版以 `flake.nix` 中的 `nekodex` Nix 包为准。Tauri 的 deb、rpm 和 AppImage 尚未完成目标系统安装验收，不作为首版承诺产物。
- Poke3 Android 10：以 arm64 签名 APK 作为直接安装和覆盖升级产物；AAB 只在未来需要商店分发时生成。
- Windows 与 macOS：保留源码兼容方向，本阶段不发布安装包。

构建输出不是发布。正式制品必须同时记录 Git 提交、应用版本、Android `versionCode`、目标架构、文件大小和 SHA-256；不得把构建缓存、密钥库或 `keystore.properties` 放进仓库。

Tauri 平台安装包会把仓库根目录的 `LICENSE` 作为资源携带；Nix 包将同一文件安装到 `share/licenses/nekodex/LICENSE`。正式制品验收必须检查目标安装包或 Nix 输出中确实存在该文件，不能只根据配置声明推断许可证已经随包分发。

发布提交完成后先运行元数据检查；传入制品路径时会同时生成包含大小和 SHA-256 的 JSON 清单：

```bash
node tools/release-metadata.mjs --require-clean
node tools/release-metadata.mjs --require-clean path/to/nekodex.apk path/to/linux-package
node tools/release-metadata.mjs --require-clean --android-apk path/to/nekodex.apk
node tools/release-metadata.mjs --require-clean --android-apk path/to/nekodex.apk \
  --expected-android-cert-sha256 PREVIOUS_RELEASE_CERT_SHA256
node tools/release-metadata.mjs --require-clean --nix-output /nix/store/...-nekodex-0.1.0
```

该工具要求 `tauri.conf.json`、Cargo workspace 和 Nix 包版本完全一致，并按 Tauri 的 Android 映射计算 `versionCode`。普通制品参数只记录大小与 SHA-256；使用 `--android-apk` 时还会通过 Android SDK Build Tools 严格检查包名、`versionCode`、`versionName`、签名有效性并记录签名证书 SHA-256。覆盖升级发布应传入上一正式版本的证书摘要；缺少 `aapt` 或 `apksigner` 时检查直接失败，不降级为“已验证”。使用 `--nix-output` 时会检查主程序、桌面文件、图标和与仓库一致的许可证，并通过 `nix path-info` 记录 NAR 哈希与大小。它不构建、签名或上传制品。

## 版本规则

唯一用户可见版本来自 `apps/shell/src-tauri/tauri.conf.json` 的顶层 `version`。Rust crate 和 Nix 包在发布提交中必须使用相同版本。

运行中的 PC 与 Android 应用会在统一页头显示由 Rust 包编译进入二进制的 Nekodex 版本。升级验收应同时核对页头版本、Android `versionCode` 和制品清单，不能只根据安装文件名判断正在运行的版本。

Android 默认使用 Tauri 的确定性映射：`versionCode = major * 1,000,000 + minor * 1,000 + patch`。每次覆盖升级都必须提升语义版本，从而保证 `versionCode` 严格递增；不要依赖生成文件中仅供非 Tauri 调用兜底的 `1`/`1.0`。

## Android release 签名

1. 在发布构建机外部生成并备份唯一的上传密钥库。
2. 将 `apps/shell/src-tauri/gen/android/keystore.properties.example` 复制为同目录下的 `keystore.properties`，填写绝对密钥库路径和密码。
3. `keystore.properties` 已被忽略；密钥库也必须位于仓库外。
4. release Gradle 任务在缺少该文件时会明确失败，避免误把未签名包当成可升级制品。debug 构建不需要发布密钥。

配置完成后才可由发布人员执行：

```bash
cargo tauri android build --apk --target aarch64
```

正式分发前还必须核对 APK 的包名为 `dev.nekodex.workbench`、签名证书与上一版一致，并记录校验和。本仓库不自动创建、轮换或上传签名密钥。

## 覆盖升级与身份

升级必须使用相同包名、相同签名证书以及更高的 `versionCode`，并采用覆盖安装；卸载后重装不是升级。

- 设备组数据库位于 Tauri 应用数据目录的 `device-v1.sqlite3`，启动时执行单向 schema migration。
- Android 设备私钥由 Android Keystore 包装，密文位于应用的 `noBackupFilesDir`。普通覆盖升级会保留它。应用同时关闭 Android 自动备份，避免系统只恢复设备组数据库却无法恢复 Keystore 私钥；卸载重装和跨设备迁移不会恢复该身份。
- 身份密钥不可导出是安全边界，不提供私钥备份功能。身份丢失后的恢复方式是以新设备身份重新加入设备组，并由现有成员撤销旧身份。
- 不支持把新 schema 的数据库降级给旧版本。升级失败时保留应用数据和错误信息，不应先卸载应用；修复版必须继续使用相同包名与签名。
- 运行中的 SQLite 可能包含 WAL 数据，不把手工复制单个 `.sqlite3` 文件描述为可靠备份。首版的可恢复缓存可由来源重新采集，成员资格依赖现有组员重新邀请。

已有正式签名版本时，发布验收才需要从该版本执行一次覆盖升级，并核对设备 ID、组 ID、成员名册和自动重连保留。首次正式发布没有旧正式版本，不为构造升级矩阵延迟首发；只需确认签名密钥已妥善保存，并完成一次全新安装和核心冒烟。已有 debug 覆盖安装证据只作为开发链路记录。

## Linux 升级

Nix 包升级替换程序闭包，不删除 Tauri 应用数据目录。升级前后需核对相同的 `device-v1.sqlite3` 与平台密钥存储仍可读取；回滚到不理解当前 schema 的旧版本不受支持。

数据库的多步 schema migration 在单个 SQLite transaction 中执行，并在提交前最后更新 `user_version`；迁移失败不会把半完成 schema 标记为新版本。这不等于支持向旧版本降级。

## 上线后观察清单

- 第二个正式版本发布时验证相同签名下的覆盖升级。
- Nix 包升级、卸载后保留用户数据的完整边界。
- Poke3 已有一次前台空闲 CPU/PSS 基线，见 `docs/performance-snapshot.md`；缓存首屏、残影、后台系统回收、长时间待机和耗电仍待验收。
- 第三节点入组及原来源节点短时不可响应后的其余节点直连已有一次 WSL 临时节点运行证据；在线撤销收敛、长时间网络分区以及 PC/Poke3 全页面数据语义一致性仍待验收。

这些条目不阻塞首次上线。仅在真实使用暴露缺陷时修改对应实现；未出现具体缺陷前不扩展测试矩阵、远程控制、云中转或其他平台发布范围。
