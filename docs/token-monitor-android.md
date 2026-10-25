# Android APK 指南

APK 是唯一产品。`apps/android/` 在构建时将 `ui/` 内的页面编入安装包；原生层连接外部数据源。最低 Android 10（API 29），当前目标为 Poke3；系统 WebView 需支持 JavaScript 模块及页面使用的浏览器 API。

## 连接与数据范围

首次打开进入连接页，通过“上页／下页”填写来源并“保存并连接”；设置中的“编辑”可修改连接，“连接”可分页查看状态及完整地址。

| 来源 | 示例 | 固定 GET 接口 |
| --- | --- | --- |
| Hub | `http://192.168.1.10:17321` | `/api/stats`、`/api/history` |
| 归档，可选 | `http://192.168.1.10:18788` | `/api/session-archive` |

两项来源独立配置地址与密钥。地址只填写 `http(s)://主机:端口`，不含路径、用户名、查询或令牌。输入留空保留同一来源的已有密钥；“清除”在保存时删除。更改来源地址时不继承旧密钥。保存接口中，`null` 表示保留同一来源密钥，空字符串表示清除。

Hub 提供汇总快照和历史，不提供完整本机会话归档。APK 仅兼容已有外部归档服务；本仓库不提供或维护服务实现。归档接口以 Bearer 密钥认证，返回含布尔值 `available`、会话对象 `sessions` 和可选时间 `capturedAt` 的 JSON；会话对象按稳定会话标识索引，读取允许的用量与展示字段。

归档按 `local-device` 处理，只代表来源电脑已有留存，不代表跨设备完整历史，不会重复累加进 Hub 总量。未配置归档、来源错误或读取失败分别提示，不把缺失当成零。会话最近观测状态不保证实时运行；APK 不提供完整对话正文或工具调用详情。

额度是上游采集快照，刷新只会重读来源；账号的 `updatedAt`、`stale` 与错误状态分别保留。百分比和余额的 0 是有效值，缺失不变成 0；费用为估算 USD 金额，不作为账单。

## 安全、缓存与刷新

Hub 原有密钥同时可授权写接口；APK 仅提供固定 GET，不能据此称上游密钥只读。需要真正限制客户端权限时，应使用外部提供的只读认证入口。

- 包内页面使用固定虚拟 HTTPS 来源，阻止外部页面、文件访问、弹窗和页面导航。原生层只转发三个固定 GET，不接受任意 URL、Cookie 或页面提供的请求头。
- 凭据使用 Android Keystore 的 AES-GCM 密钥加密，经 AtomicFile 原子写入 `noBackupFilesDir`，备份关闭。页面只能读取地址、配置版本和密钥是否已保存；APK 不内置个人地址或凭据。
- 网络不跟随重定向、不使用系统代理，HTTPS 正常校验证书；HTTP 没有传输加密。连接和读取各有 8 秒超时，响应上限 8 MiB。
- 统计、历史、归档分别缓存最近成功的展示字段。切换来源清除数据缓存并丢弃旧请求结果，保留显示和刷新偏好；断线明确标记缓存。
- 默认 60 秒刷新，可选 5 分钟或手动。退后台停读并断开进行中的原生请求，回前台恢复；没有常驻服务或唤醒锁。返回键先退出输入、字段详情或设置，再回桌面。

## 构建与签名升级

需要已有 JDK 17+、Android SDK platform 36 及匹配 build-tools；Gradle wrapper 为 8.14.3，Android Gradle Plugin 为 8.11.0。首次解析依赖需要网络或已有缓存。

Debug 构建与模型／原生策略检查见 [README](../README.md)。Debug 输出 `output/android/token-monitor-eink-debug.apk` 及 SHA-256 文件，包名为 `dev.tokenmonitor.eink.debug`，开启应用与 WebView 调试。

Release 使用长期保存的私有签名配置，字段见 [keystore.properties.example](../apps/android/keystore.properties.example)。真实密钥及密码放在仓库外，在仓库根目录执行：

```bash
export TOKEN_MONITOR_SIGNING_FILE=/private/path/release.properties
tools/build-eink-apk.sh release
```

输出 `output/android/token-monitor-eink-release.apk` 及 SHA-256 文件；包名 `dev.tokenmonitor.eink`，关闭应用及 WebView 调试。缺少签名配置时构建失败。覆盖升级须保留相同包名和签名，并增加 `versionCode`；勿丢失发布密钥。

## 历史验收与当前边界

本次目录收敛已通过 Release 构建、原生连接策略检查及 31 项界面模型测试；包内 11 个界面资源与源码逐文件一致，未编入测试文件。模拟 Android 接口的首次连接、保存配置及三路读取通过；7 种尺寸下的连接／状态页、六种重点色、五页导航及滚轮拦截共 75 个检查状态无溢出、滚动或脚本错误。这些使用模拟数据，尚未在 Poke3 重新安装本次构建。

目录收敛前的 Debug／Release 1.0.0 已完成构建、签名及包内资源一致性检查，并在 Poke3 Android 10／WebView 108、约 662×453 CSS 视口安装运行。

历史真机通过局域网外部只读网关端口 18788 读取统计、历史和归档。当时 Hub 端口 17321 未获防火墙放行，Wi-Fi 直连超时；直连 Hub 仅通过 USB reverse 验证，不能算作 Wi-Fi 直连验收。

历史版本还通过连接／状态页 99 个模拟尺寸状态、真机键盘约 662×225 视口、返回键、后台拒读／前台恢复、Release 冷启动恢复加密连接及网关断线缓存恢复。模拟尺寸不等于其他实体设备实测。

Poke3 若冻结新安装应用，可在设备应用管理中解除冻结，无需清除数据。长期耗电、物理残影与实体彩屏尚未测试；外部来源进程需自行启动和维护。
