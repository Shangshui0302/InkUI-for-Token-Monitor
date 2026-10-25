# Token Monitor 墨水屏浏览器方案

日期：2026-10-25。当前交付是可在浏览器打开的完整只读看板，复用 Token Monitor 的采集能力，并可选读取该电脑已有的会话用量归档。此方案取代旧自建采集器的 M2/M3/M4 开发计划。

## 运行链路

```text
Token Monitor 采集与账号管理 ── GET /api/stats、/api/history（Hub 密钥） ─┐
Token Monitor 本机 SQLite 会话用量归档（可选，只读含 WAL） ────────────────┤
                                                                           ↓
tools/token-monitor-eink.py（静态页面 + 固定只读路由与字段白名单）
        ↓ GET /api/stats、/api/history、/api/session-archive（查看令牌）
apps/eink/（电脑或 Poke3 浏览器）
```

服务仅使用 Python 标准库；前端是无需构建的 HTML、CSS、JavaScript。它不读取 Codex 数据目录、不采集供应商接口、不管理账号、不启动 Token Monitor，也不依赖旧 Tauri、Rust、App Server 或 8788 设备组。启用 `--session-archive` 时，只对显式给出的现有 Token Monitor SQLite 归档执行只读查询（包括 WAL），按字段白名单返回看板所需数据；不会改写数据库。

启动和参数见 [README](../README.md#启动)。页面服务默认端口 8766，现场运行指定为 18788；Hub 地址默认为 `http://127.0.0.1:17321`。环境变量为 `TOKEN_MONITOR_URL`、`TOKEN_MONITOR_SECRET` 和 `EINK_VIEW_TOKEN`；Hub 密钥还可通过 `--hub-credentials-file` 从现有 Token Monitor 凭据 JSON 的 `credentials.hub.hostSecret` 字段读取，或通过私有文件参数读取。完整看板启动时可增加 `--session-archive "/home/lishangshui/distrobox/arch/.config/Token Monitor/session-usage-archive.sqlite"` 启用本机归档读取。非回环监听必须配置独立查看令牌。查看令牌成功认证后保存在当前标签页的 `sessionStorage`，不会写入 `localStorage`。

## 接口与显示语义

本轮检查已安装 Token Monitor 0.68.0 的提取源码：`src/hub/server.js` 的 `getStats` 和 GET 路由、`src/shared/http.js` 的认证逻辑、`src/shared/limits/core.js` 的标准化与去重逻辑。该版本 Hub 不提供静态网页；认证使用 Bearer 或 `x-token-monitor-secret`。本服务只使用 Bearer，并禁止上游重定向和环境代理，避免 Hub 密钥被转发到其他目的地。

成功 stats JSON 直接包含 `devices` 和 `limits.providers`，没有 `stats` 外层包装；页面使用已由 Hub 跨设备去重的 `limits.providers`，不再次按 provider 合并，保留不同账号。`/api/history` 提供 Token Monitor 汇总历史；`/api/session-archive` 是可选的本机 SQLite 归档读取。累计会话只涵盖这台电脑已留存的归档记录，不代表跨设备完整历史。看板累计汇总仍以 Hub 为准，不把本机归档再叠加到 Hub totals。

| 字段 | 页面含义 |
| --- | --- |
| `provider`, `accountLabel` | 供应商、可选账号标签 |
| `status` | `ok`、停用、未配置、认证失败、额度受限、采集受限、不可用或错误 |
| `stale` | 上游对该账号采集记录的过期判断 |
| `updatedAt` | 该账号采集时间；根对象的更新时间不能替代它 |
| `windows[].remainingPercent` | 剩余百分比，0 是有效数据 |
| `windows[].usedPercent` | 无剩余百分比时，按 100 减去已用值换算 |
| `windows[].remaining`, `currency` | 余额，例如 DeepSeek 的 CNY；缺失值不转成 0 |
| `windows[].resetsAt` | 按浏览器时区显示绝对重置时间；缺失时显示未提供 |

额度与余额的缺失、来源错误、来源过期、历史不可用、归档不可用以及浏览器断线分别提示。HTTP 请求成功只表示拿到了快照，不证明供应商刚刚采集成功。页面不把 Hub 用量解释为 Codex 任务或审批实时状态，也不声称完整复制会话对话正文或工具调用详情。

## 看板页面与数据边界

五个页面分别是总览、额度、会话、活动和明细。用量周期通过日／月／累计按钮切换；看板展示 token 数、估算 USD 费用、输入/输出等分项和速度，并按现有数据展示 models、clients、projects、devices。估算费用表示上游记录对应的估值，不作为账单金额。

墨水屏采用固定视口 shell：顶栏显示状态、时段按钮和五页导航，底部提供固定翻页按钮。页面内容按可用宽高调整行数，宽屏总览采用两列；所有滚动均关闭。长字段从列表行按钮进入分段详情，通过按钮翻页并可返回原页。会话页使用“全部／运行中”按钮筛选，不提供搜索或固定条数分页。活动页通过按钮切换全年分阶热力、趋势、每日与每月内容，不依赖日期输入控件。设置使用独立满屏页面和按钮标签切换显示、刷新、连接、令牌与缓存；令牌输入用于初次连接，较长说明收在来源详情中。界面提供黑白和彩色两套配色，使用细线和清楚的数字层级，无动画；正常状态只显示在线状态和时间，错误与缓存提示保持简短。控件尺寸随可用屏幕自适应，不统一保证 48 CSS px。

会话归档只提供 Token Monitor 已留存的会话用量/必要展示元数据；它不是跨设备完整历史，也不包含可供此页面完整浏览的对话正文或工具调用详情。归档中最近观测状态不能被解释为当前实时运行状态。

Hub 的 `/api/stats` 和 `/api/history` 仍是总体额度及汇总数据来源；本机 `session-usage-archive.sqlite` 只补充该设备已留存的会话条目。看板汇总不会把归档内容重复累加进 Hub totals。归档读取由启动参数显式启用；未配置时 `/api/session-archive` 明确返回不可用状态。

## 黑白和彩色墨水屏配色

“设置 → 显示”提供黑白／彩色按钮，默认黑白。彩色提供蓝、青、绿、琥珀、玫瑰、紫六种重点色按钮，默认蓝色；模式与重点色分别保存在本机 `localStorage`，切回黑白仍记住彩色偏好。旧版只有彩色模式记录时，自动采用蓝色；无效色系也回退为蓝色。两种模式使用同样的固定视口、分页及字号，切换不会改变当前页面或页码。黑白所有设计色均为中性灰；彩色保持白底、黑色正文及中性灰分隔，亮色重点色用于选中按钮实底、比例条和图表；选中按钮使用黑色文字，白底强调文字和额度数字使用同色系深色以维持清晰度。赭黄／暗红仅提示低额度或较高上下文占用。百分比和状态文字始终保留，不以颜色作为唯一信息。

常见能力参考 E Ink 官方 [Carta 1000 的 16 级灰度](https://www.eink.com/brand/detail/Carta1000)及 [Kaleido 3 的 16 级灰度／4096 色](https://www.eink.com/brand/detail/Kaleido3)。本 UI 黑白色阶采用 `#ffffff` 至 `#000000` 的 16 个离散灰阶；各彩色色系选用明亮、高饱和重点色，16 阶热力色阶由白色递进到饱和彩色，避免近黑的尾部，RGB 通道值均取 17 的倍数。这个 16×16×16 软件网格是设计选择，不代表厂商的色彩映射，也不是实体屏幕校色结果。热力图按窗口内用量峰值分 16 档，0 档留白、非零最少 1 档；趋势不填充渐变。

主文字、次要文字和语义数字针对其背景的计算对比度不低于 4.5:1；这是 CSS 色值对比，不能替代实体墨水屏的反射、残影和刷新模式测试。页面不使用动画或过渡，Chrome 85 的焦点轮廓也使用当前主题颜色。

## 页面刷新与缓存

默认 60 秒，可选 300 秒或手动；隐藏页面暂停，回到前台读取。stats、history、session archive 分别更新，某一路失败时保留该路最近成功快照并明确标记；成功与失败状态互不覆盖。刷新轮次不重叠，先读取用量，再并行读取需要更新的历史和归档；每个请求 10 秒超时。展示内容未变化的区域保留 DOM，禁用动画和过渡，使用深色高对比文字和紧凑信息布局。

stats、history 和 archive 的允许展示字段及各自读取时间分别保存在浏览器 localStorage；刷新间隔、主题模式和重点色选择也保存在 localStorage。查看令牌仅在成功认证后保存在当前标签页的 sessionStorage，不保存到 localStorage。缓存会保留标题和项目名等必要显示字段，但不保存凭据、内部账号 key、会话对话正文或工具调用详情。读写被浏览器拒绝时，页面仍能读取在线数据。重新加载时分别恢复各类缓存；清除缓存会分别清除三类数据，后续成功读取会各自重新保存。

本版没有 service worker：电脑页面服务本身停机时，已经打开的页面仍可显示缓存；全新加载页面仍需要服务可达。不能将此描述为离线可安装的 PWA。

## 认证与错误

静态页面和 `/healthz` 无需认证。`/api/stats` 与 `/api/history` 校验查看令牌后由服务使用 Hub 密钥向固定来源读取；配置归档后 `/api/session-archive` 同样要求查看令牌，并只对启动参数指定的数据库进行只读查询。路由固定、字段白名单，无目录浏览、任意 URL 代理、远程设置或写接口。查看令牌不得与 Hub 密钥相同，浏览器不会接收 Hub 密钥。

| 结果 | 含义 |
| --- | --- |
| 401 `unauthorized` | 浏览器查看令牌缺失或错误 |
| 502 `hub_unauthorized` | 电脑端 Hub 密钥错误 |
| 503 `hub_unavailable` | Hub 未启动、不可连接或请求超时 |
| 502 `hub_http_error` | 上游返回其他错误或重定向 |
| 502 `hub_invalid_response` | 不是预期 JSON 数据 |
| 502 `hub_response_too_large` | 单次响应超过 8 MiB |

服务不打印请求路径、请求头和上游错误正文。`/healthz` 只证明本服务存活。

## 验收记录与待办

2026-10-25 重点色色系追加验收：

- 随后按用户要求改为亮色：六套选中按钮采用亮色实底，比例条和图表同步提亮；白底小字使用独立深色。6 项主题测试通过，Chrome 85 实际渲染核对六色按钮填充、文字与 16 阶预览，蓝色热力色值一致。布局与交互尺寸未改，复用此前尺寸验收。截图 `output/playwright/bright-picker-preview.png`、`bright-blue-overview.png`、`bright-blue-heatmap.png`，记录 `bright-palette-results.json`。

- 彩色版加入蓝、青、绿、琥珀、玫瑰、紫六种重点色，统一白底黑字和中性灰分隔；模式与重点色独立记忆。窄屏色系按钮排为两行，极矮屏省略辅助色阶预览，按钮仍可访问。
- Poke3 Chrome 85 实际触控六种色系切换，检查各色总览、热力图、重载记忆以及切回黑白隔离；总览仍为两列、12 条。六色总览截图 `output/playwright/accent-*-overview.png` 已查看，彩色截图仍仅证明浏览器渲染。
- 11 种模拟尺寸（仅新增设置页，128×128 至 1920×1080）×6 色，共 66 个状态无滚动或内容溢出；记录为 `output/playwright/accent-layout-results.json`。前一次长流程在最后一步遇到调试连接断开，恢复连接后完成集中尺寸复核。
- 修复初次读取完成时收起用户正在操作的显示页：只有成功提交令牌且仍停留在令牌页时才自动返回。以延迟真实读取验证“加载中选色保持打开”“令牌成功自动返回”“提交后转到显示页不被关闭”，记录为 `output/playwright/accent-refresh-regression.json`。
- 6 项主题测试与 20 项既有数据模型测试通过；语法与 diff 检查通过。未增加滚动、动画或自由颜色输入控件。

2026-10-25 配色与视觉优化验收：

- 优化前的按钮分页版本已保存为提交 `0cc5bc1`（`feat: add paginated Token Monitor e-ink dashboard`）；后续视觉与配色修改在同一 `feature/poke3-token-monitor` 工作树内继续，未 push。
- 两套主题共用布局；Poke3 Chrome 85 的真实 773×472 CSS 视口仍呈现两列、12 条首屏总览。主导航、数字字重、次要文字、细分隔线和额度／分布／上下文比例线统一样式。原有汇总和字段详情均保留。
- Chrome 85 模拟 10 种尺寸，150 个页面状态和 40 个图表状态均无根滚动、内容溢出或脚本异常。新增显示页又在黑白／彩色两套主题下各测 6 个尺寸，共 12 个状态，包括 128×160；16 档预览、切换按钮与主题重载记忆通过。颜色和 SVG 色阶已核对浏览器实际渲染值，修复了原内联色块样式被内容安全策略阻止的问题，未放宽安全策略。
- 真机触控通过上／下页、详情翻页与返回原页、刷新保留会话页码、全部／运行中筛选、每日详情、刷新间隔切换；滑动尝试后 window、dashboard、列表偏移均为 0。旧 Chrome 默认彩色焦点轮廓改用主题色。
- 设备唤醒初期 Wi-Fi 未连接，先通过临时 USB reverse 验证布局；随后 Wi-Fi 自动恢复。删除 reverse 后，Poke3 实际通过 `http://10.229.113.165:18788/` 读取 stats、history、session archive，三路均 HTTP 200。最终保留黑白版局域网页面，调试 forward 验收后移除。
- 13 项 Python HTTP 测试、24 项 Node 测试通过；新增主题测试覆盖无效选择回退、热力零值与边界、16 阶递进／软件量化以及文字对比度。截图 `output/playwright/theme-{bw,color}-{overview,heatmap,trend,settings}.png` 已查看，检查记录在同目录 `theme-result.json` 和 `button-*.json`，均不纳入 Git。
- 彩色截图仅证明浏览器的彩色渲染，Poke3 是黑白屏，本轮没有实体彩色墨水屏验收。未进行长期残影或耗电测试。

2026-10-23 本轮固定视口验收：

- 固定视口页面结构和按钮导航已实现；关闭所有滚动，长值改为分段详情翻页并可返回原页。搜索和固定 20 条分页已移除，改为全部／运行中筛选；时段、活动视图和设置均使用按钮操作。
- Chrome 85 的 Poke3 横屏 CSS 视口 773×472 已观察到 12 条、两列的首屏总览，页面根节点未溢出。实际触控已通过：上/下页、会话详情翻页及返回原页、刷新保留页码、全部/运行中筛选、每日用量详情、刷新间隔切换。尝试滑动后 window、dashboard 和列表内容区偏移均保持 0。
- Chrome 85 的模拟视口检查覆盖 773×472、572×800、320×568、240×320、320×240、160×200、1920×1080、128×160、240×180、320×180，共 140 个页面状态；图表进一步检查 40 个状态。未发现根节点滚动、内容区溢出或 JavaScript 异常。较小屏幕减少每页条数，极矮屏把总量也纳入分页；长值分段详情可逐页读完。模拟尺寸不是对应物理设备的实测。
- 最新截图 `output/playwright/button-overview.png`、`button-heatmap.png`、`button-trend.png` 已人工查看；宽屏活动页在热力图下复用空余高度展示45日趋势。布局及触控结果保存在 `button-layout-matrix.json`、`button-layout-small.json`、`button-chart-matrix.json` 和 `button-touch-result.json`（均位于同一忽略目录）。20 项数据模型测试通过，前端语法与 diff 检查通过。

此前真机验收记录（旧版交互，2026-10-23）：

- 新隔离分支 `feature/poke3-token-monitor` 中的本轮代码来自前一日 `feature/token-monitor-eink` worktree 草稿的复制；原草稿 worktree 未修改。
- Poke3 真机（USB serial `82CCF4BF`，Android 10）在用户当前锁定的横屏方向实际访问 `http://10.229.113.165:18788/`。设备物理分辨率为 1072×1448、density 300；Chrome `org.chromium.chrome` 85.0.4178.0 页面 CSS 视口为 773×472。通过真实 Wi-Fi 地址读取 Antigravity、Codex、DeepSeek 三家 provider，全部返回 `status=ok`、`stale=false`。设备 Wi-Fi 地址为 `10.229.113.145`。`adb reverse --list` 为空；验证时仅配置调试浏览器的 `19222` forward，验收后移除；因此该结果是 Wi-Fi 局域网访问，不是 USB reverse 转发。
- Poke3 横屏 CSS 视口 773×472 下，旧版完整总览页面呈现且无横向溢出。完整看板截图 `output/playwright/full-overview.png` 与 `full-activity.png` 已人工查看。旧版五个页面及模型/工具/项目/设备明细均无横向溢出；当时的 48 CSS px 控件尺寸不适用于当前自适应布局。日/月/累计总量与同一快照一致。
- 旧版会话页通过真机触控进入；当时的累计归档搜索、每页 20 条翻页、刷新保留搜索状态与展开详情，以及活动日期触控均已验证。这些交互已由本轮按钮式浏览替换，不作为当前功能验收。
- 页面重载后查看令牌恢复。清除缓存一次清空三份数据，刷新后恢复。实际停止本轮页面服务后，页面显示“连接失败”与缓存说明，历史图表和累计会话仍可查看；重启后刷新恢复“已连接”。断线截图为 `output/playwright/full-disconnected.png`。
- 13 项 Python HTTP 测试通过，包含凭据读取安全、只读固定路由、历史接口和会话归档只读/字段白名单检查。
- 看板模型 12 项和原额度模型 8 项 Node 测试通过，共 33 项 Python/Node 测试。
- 本次会话在当前 worktree 前台运行页面服务，以 `--hub-credentials-file` 从现有 `/home/lishangshui/distrobox/arch/.config/Token Monitor/credentials.json` 读取 Hub 密钥，以 `--session-archive "/home/lishangshui/distrobox/arch/.config/Token Monitor/session-usage-archive.sqlite"` 启用本机历史归档读取，并从当前仓库忽略 Git 的 `.codex-monitor/view-token.secret` 读取查看令牌；未复制密钥到代码或文档。服务以 `--host 0.0.0.0 --port 18788` 运行。

2026-10-22 已完成：

- 8 项 Python HTTP 测试：认证隔离、固定只读路由、静态资源、上游失败、重定向拒绝、格式检查、Hub 不可达和局域网配置边界。
- 8 项 Node 模型测试：百分比／余额零值、空值、错误与多账号、过期状态、格式检查、缓存字段筛选、空标签回退和账号排序；前端语法检查通过。
- 本机浏览器用模拟数据确认三家卡片、多账号、零余额、过期提示、错误认证、断线缓存、重新加载恢复与清除缓存；HTML 样式账号字符串按纯文本显示。600×800 首屏可读到 Codex 额度及 DeepSeek 标题，320px 视口在设置收起／展开时均无横向溢出。
- 启动已安装的 Token Monitor 后，本机真实 `/api/stats` 经页面服务返回 HTTP 200，三个 provider（Antigravity、Codex、DeepSeek）均 `status=ok`、`stale=false`，窗口数分别为 4、1、1；浏览器实际展示成功。此记录证明 2026-10-22 15:40 左右的本机链路，不代表长期刷新或 Poke3 验收。
- 本机预览监听 `127.0.0.1:8766`，Hub 密钥在服务进程内从既有凭据文件读取，没有复制到仓库、页面或日志。

仍未覆盖：

- 长期耗电与物理残影未测。系统启动未配置。
- 本轮未进行 NixOS/Home Manager 修改、系统 rebuild 或 APK 安装；现场端口已有防火墙放行，本轮未修改防火墙。

旧 2026-10-11 三家供应商读取记录仅证明当时 Token Monitor 可用；不构成本次界面或当下额度的验证。现场服务与真实 Hub 链路、缓存清除及断线恢复的真机记录属于此前界面版本；本轮固定视口布局已完成上述多尺寸与按钮触控验收。系统启动未配置；如需开机启动，后续另行配置和验证。
