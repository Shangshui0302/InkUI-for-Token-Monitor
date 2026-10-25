# InkUI for Token-Monitor：Android 指南

InkUI 是独立开发的非官方 Token Monitor 客户端，Android APK 是本仓库唯一产品。包内界面随 APK 发布，原生层读取外部 Hub；账号管理、采集和跨设备汇总由 [Token Monitor](https://github.com/Javis603/token-monitor) 提供。本仓库不开发或维护 Hub、归档服务，也不直接查询各服务商 API。

最低 Android 10（API 29），当前主要适配 BOOX Poke3。系统 WebView 需支持 JavaScript 模块及页面使用的浏览器 API。安装入口见 [README](../README.md#安装与连接)。

## 连接与数据范围

首次打开进入连接设置。设置分为连接、外观、数据、屏幕四项；常用 Poke3 视口同屏填写地址与密钥，小屏或键盘压缩后按步骤分页，不滚动表单。填写后先“测试当前输入”，成功后仍需“保存并连接”；测试不会改写配置。取消修改恢复已保存配置，保存失败保留输入。

| 来源 | 示例 | 固定 GET 接口 |
| --- | --- | --- |
| Hub | `http://192.168.1.20:17321` | `/api/stats`、`/api/history` |
| 归档，可选 | `http://192.168.1.20:18788` | `/api/session-archive` |

两项来源独立配置地址与密钥。裸 IP／主机名自动补 `http://`，Hub 默认端口为 `17321`；明确填写协议或端口时按原输入连接。完整地址不含路径、用户名、查询或令牌。`localhost` 指向 Android 设备本身，电脑服务应填写电脑的局域网地址，并确保 Hub 监听与防火墙允许访问。

同一来源的密钥输入留空表示保留，“清除”在保存时删除；更改来源地址不继承旧密钥。保存接口中 `null` 表示保留同一来源密钥，空字符串表示清除。测试使用未保存的输入；修改地址、来源或离开页面会取消测试。认证失败、超时、找不到主机、拒绝连接及接口不符分别提示。

Hub 提供汇总快照和历史，不提供完整本机会话归档。APK 仅兼容已有外部归档服务：以 Bearer 密钥认证，返回含布尔值 `available`、会话对象 `sessions` 和可选时间 `capturedAt` 的 JSON；会话对象按稳定会话标识索引。归档按 `local-device` 处理，只代表来源电脑已有留存，不重复累加进 Hub 总量。未配置、来源错误或读取失败分别提示。会话最近观测状态不保证实时运行，APK 不提供完整对话正文或工具调用详情。

额度是上游采集快照，刷新只重读来源；保留 `updatedAt`、`stale` 与错误状态。百分比和余额的 0 是有效值，缺失不变成 0；费用为估算 USD 金额，不作为账单。活动曲线按真实日期定位，以平滑实线连接已有记录，保留真实零值和首尾留白；没有记录不等于采集失败，接口未提供逐日覆盖状态时也不能认定真实零用量。

## Provider 额度覆盖

以 [Token Monitor v0.78.0 的额度注册表](https://github.com/Javis603/token-monitor/blob/v0.78.0/src/shared/limits/providers.js) 为基线，固定上游 commit `d52e0b9cf970a03a30a5f5a1e9139693bb211cd8`，逐家核对 30 个额度 provider 的 mapper、共享 schema 和上游显示语义；这不是采集客户端数量。下表各分支表示源码允许的情况，不表示一个账号一定同时拥有所有额度。

| Provider | 已适配的额度与特殊字段 |
| --- | --- |
| Claude | 会话、每周、具名模型／促销池；消费额与可选上限、预付余额、重置券及每券明细 |
| Codex | 按真实时长划分的会话／每日／每周／每月；附加具名桶、窗口标识、重置券 |
| OpenCode | Go 的会话／周／月额度与 Zen 余额；保留每个窗口的网页或本地来源 |
| Cursor | 旧版请求次数、模型池、Overall、Grok Bot、团队金额池、按量消费；新版 Included usage 与预付余额 |
| Antigravity | Gemini、Claude/GPT 等组的短周期与周窗口；旧模型池、空额度、验证账号状态 |
| Cline | 计划周期、CREDITS 点数余额及 USD 消费额，分别显示并保留单位 |
| Factory Droid | Standard／Core 的 5 小时、周、月额度；附加池、额外 USD 余额；旧 Standard／Premium Token 月池 |
| Kimi | Code 与会员共享周期；月度组成说明保留，百分比不替代原说明 |
| Grok | 周、月及其他账期标签；保留上游提供的百分比和边界 |
| GitHub Copilot | Premium／Chat 百分比与超额重置说明；缺少上限不擅自标为不限额 |
| Zed | Token Spend 金额与 Edit Predictions 次数；明确的不限额状态 |
| Command Code | 短周期 USD 限额、每月金额池和 Top-up；未知总额仍显示余额，不伪造刻度 |
| Xiaomi MiMo | Console Token Plan、钱包、赠送／现金拆分和消费跟踪；Desktop Membership 独立产品行；过期计划元数据 |
| GLM | 5 小时、周、MCP、区域货币余额；ZCode 模型 Token 池、每日／一次性配额及重置／到期／混合边界 |
| GLM Team | 团队 5 小时、周与 MCP 池 |
| Kiro | Credits／Bonus 点数；Overage 的已用点数和预估 USD 费用，后者不当成剩余额度 |
| WorkBuddy | 个人点数、企业点数或明确不限额；可用量与已用量；应用会话加密状态 |
| Qoder | 自有／共享点数池的已用、上限和剩余 |
| DeepSeek | 原币种余额、观察所得消费与跟踪起点／部分月份口径 |
| Devin | 每日／每周配额与额外 USD 余额，窗口可独立缺省 |
| Minimax | Token Plan 短周期／周额度或 API 余额；CN／EN 货币及缺失值 |
| TypeSafe | USD 余额、每批赠额有效期、月消费估算与服务商用量统计 |
| OpenRouter | Key 的日／周／月／总金额限制及账户 Credits；无上限、部分字段缺失与余额分开处理 |
| Volcengine | Coding 的 5 小时／周／月；Ark 请求数限额；无单位的用量回退保留数值并明确注明 |
| Ollama | 真实 1 小时或 5 小时窗口与周额度，不统一写成 5 小时 |
| Trae CN | 有效资源包汇总点数、已用原数值和可用点数；不凭空补统一到期时间 |
| Alibaba Cloud | 个人多周期、团队月池、仅上限或仅百分比、有效但没有活动套餐的空窗口 |
| StepFun | 5 小时／周额度或 Credit 百分比；缺少原金额时不伪造金额 |
| Notion AI | 6 小时 Rolling 与每月点数池，保留原时长、使用量和重置时间 |
| Third-party APIs | NewAPI 账号、NewAPI Token、Sub2API、Custom；余额、Token quota、不限额、零余额、组／有效期与服务商用量明细 |

实现边界是认证 `GET /api/stats` 已返回的字段。额度页显示主要数值、单位和可用刻度；点开每一项，可按按钮分页读取窗口的 used／limit／remaining、两种百分比、实际周期、重置说明、来源、附加标记、窗口标识、边界类型和详细说明，以及所属账号资料。余额明细包括消费、赠送／现金、计划用量、有效期与分批赠额；重置券包含数量、有效期和各券状态；服务商用量包含请求、Tokens、缓存、费用与延迟。这些服务商侧统计不重复加进会话总量。

`metric=credits` 的金额／点数优先保留原值，百分比另作辅助；`metric=spend` 显示已消费。`showMeter=false` 时不画刻度；没有总额的余额不估算百分比。不把缺失当零，也不把未提供上限当成不限额。Kiro Overage、CREDITS 点数、Zed 次数与金额等有明确的服务商规则；未知单位保留为空并在必要时提示。完整数值与主卡片的缩写分开，微小正余额不会四舍五入成零。

覆盖夹具在 `apps/android/ui/fixtures/provider-limits-v0.78.json`：52 个合成源码分支，附固定上游版本、commit 和各 mapper 行号；包含 30 家服务商、MiMo 双产品、四种第三方 adapter、特殊字段与错误状态。值均为合成数据，不包含真实账号响应。`test_provider_catalog.mjs` 验证投影／缓存往返和语义，`test_quota_presentation.mjs` 验证显示与边界情况。

上游限制：0.68.0 与 0.78.0 的共享额度字段结构兼容，但 0.68.0 不认识 Notion，部分 Cline／WorkBuddy 多账号聚合行为也较旧；APK 无法恢复已被旧 Hub 丢掉的行。火山 Ark 的一条回退分支把 Token 总量放在名为 Requests 的字段，却没有单位标记，客户端不会猜成请求次数或 Token 数。Copilot 没有传递明确的不限额标记时只能保留未知状态。原始服务商没有传到 Hub 的字段，前端同样无法恢复。

## 显示、分页与设备状态

主导航固定为额度、会话、活动、明细四页，支持日／月／累计视图。界面固定视口、无滚动、无动画；额度按账号信息量分配矩形卡片，完整字段通过详情按钮分页读取。服务商筛选菜单与长详情同样分页，账号和窗口不按固定数量截断，只渲染当前页图形。

内容区左划下一页、右划上一页，抬手后整页替换；与翻页按钮一致，首尾停止，不跨主栏目跳转。短距离、纵向、明显斜向、长按、多指及取消手势不翻页；输入框保留输入操作，屏幕边缘留给系统返回手势。

首次启动默认全屏，顶栏可切换并记住选择。Android 10 使用沉浸式系统栏与键盘 `adjustResize`；Android 11+ 使用 WindowInsetsController 处理系统栏、键盘与刘海边距，依据 [Android 沉浸模式](https://developer.android.com/develop/ui/views/layout/immersive) 与 [WebView insets](https://developer.android.com/develop/ui/views/layout/webapps/understand-window-insets)。

顶栏显示本地时区的 24 小时时间、电量／充电与网络链路。网络链路和 Hub 连接独立：局域网存在但外网未验证，不会被误判为 Hub 断开。缺失状态显示未知；不读取 SSID、IP、位置或网络标识。原生前台监听仅使用普通 `ACCESS_NETWORK_STATE` 权限，无位置权限或后台监听；同值事件和设置遮挡的顶栏变化不请求刷新。

黑白模式采用 16 阶灰度；亮色模式提供蓝、青、绿、琥珀、玫瑰、紫六种重点色。配色可用于普通彩屏，但实体彩色墨水屏尚未验收。

## 墨水屏刷新

数据读取与屏幕刷新分别处理。原生刷新等待 WebView 提交新画面，同帧变化合并、数据变化短暂节流；相同画面不额外刷新，后台取消待执行工作。

- Poke3 默认 GC16 局部模式，保留 16 级灰度；每 6 次翻页或累计 30 次实际局部更新请求全刷。配色切换、恢复前台和“设置 → 屏幕 → 清屏”请求全刷，没有数据变化时不按定时器空刷。
- 适配基于 Poke3 Android 10 框架公开的 `View.getDefaultUpdateMode()`／`setDefaultUpdateMode(int)` 与普通 `invalidate()`，只为本次重绘设置模式，随后恢复；不修改系统刷新设置，不引入厂商 SDK。灰度局部／全刷语义参考 [Onyx 官方更新模式说明](https://github.com/onyx-intl/OnyxAndroidDemo/blob/ec512c79c0d39c1e294214d36580ce4c914bbea6/doc/EPD-Update-Mode.md)，固件指纹与框架校验值记录在原生适配类中。
- 未验证机型、缺少接口或调用失败时由系统管理刷新，并禁用硬件清屏按钮。不用网页黑白闪屏模拟硬件全刷；其他机型的专用彩色刷新波形尚未适配或实测。

## 连接、缓存与刷新策略

Hub 原密钥同时可授权写接口；APK 只转发上述三个固定 GET，不接受任意 URL、Cookie 或页面提供的请求头。需要真正的权限隔离时，应自行提供只读认证入口。HTTPS 正常校验证书；HTTP 没有传输加密，应在可信网络使用。

包内页面使用固定虚拟 HTTPS 来源，阻止外部页面、文件访问、弹窗和导航。凭据以 Android Keystore 的 AES-GCM 密钥加密，经 AtomicFile 写入 `noBackupFilesDir`，关闭备份；页面只能读取地址、配置版本和密钥是否已保存，不内置个人连接配置。

网络不跟随重定向、不使用系统代理；连接与读取各有 8 秒超时，响应上限 8 MiB。未保存连接测试使用相同校验，10 秒总截止，异步运行且仅有一个有效测试；退后台取消，结果不返回响应正文或密钥。

统计、历史与归档分别缓存最近成功的展示字段，断线明确标记缓存。额度详情可包含账号姓名／邮箱和账务展示信息；私有 ID、密钥、Cookie、请求头及任意原始响应不进入页面缓存。切换来源清除展示缓存并丢弃旧请求，保留外观和刷新偏好；清缓存需再次确认，不清除连接配置。

默认每 60 秒读数据，可选 5 分钟或手动。退后台停读并断开进行中的请求，回前台恢复；没有常驻服务或唤醒锁。返回键先退出输入、详情或设置，再回桌面。

## 构建与签名升级

需要已有 JDK 17+、Android SDK platform 36 及匹配 build-tools。Gradle wrapper 为 8.14.3，Android Gradle Plugin 为 8.11.0；首次解析依赖需要网络或已有缓存。在仓库根目录执行：

```bash
export JAVA_HOME=/path/to/jdk
export ANDROID_HOME=/path/to/android-sdk
tools/build-eink-apk.sh debug
```

Debug 输出 `output/android/inkui-debug.apk` 及 `.sha256` 文件，包名 `dev.tokenmonitor.eink.debug`，开启应用与 WebView 调试。模型及原生策略检查入口见 [README](../README.md#开发)。

Release 使用长期保存的私有签名配置，字段见 [keystore.properties.example](../apps/android/keystore.properties.example)。密钥及密码放在仓库外：

```bash
export TOKEN_MONITOR_SIGNING_FILE=/private/path/release.properties
tools/build-eink-apk.sh release
```

输出 `output/android/inkui-release.apk` 及 `.sha256` 文件，包名 `dev.tokenmonitor.eink`，关闭应用及 WebView 调试。缺少签名配置时构建失败。覆盖升级必须保持相同包名与签名，并提高 `versionCode`；请长期保留发布密钥。

首个公开版本为 **v1.2.0**，沿用此前内部迭代版本号，`versionCode` 为 5。发布资产命名为 `InkUI-for-Token-Monitor-v1.2.0.apk` 和 `SHA256SUMS`；安装下载与校验值以 [发布页](https://github.com/Shangshui0302/InkUI-for-Token-Monitor/releases/latest) 为准。

## 当前验证范围

v1.2.0 发布源码已通过 83 项 JavaScript 测试，以及原生连接策略、异步连接测试、刷新策略、Onyx 适配和设备状态检查。浏览器模拟使用合成数据与替身桥，覆盖分页、字段详情、设置、键盘压缩视口、焦点和连接错误；这类检查不代表其他实体设备实测。

Poke3 Android 10 已验证真实地址／密钥输入、测试与保存，并通过 Wi-Fi 直连 Windows 上的 Token Monitor 0.78.0 Hub；四个主页面已打开核对，退出进程后冷启动可恢复加密连接。本次公开发布包（versionCode 5）已覆盖安装，冷启动后保留配置并恢复在线数据；设备内 APK 与发布文件的 SHA-256 一致，包内 18 个界面资源与源码逐字节一致，APK v2 签名验证通过。安装下载与校验值以发布资产为准。

物理刷新波形、长期残影、耗电、长期连接和其他实体机型／彩屏尚未完成验收。Poke3 如冻结新安装应用，可在设备应用管理中解除冻结，无需清除数据。
