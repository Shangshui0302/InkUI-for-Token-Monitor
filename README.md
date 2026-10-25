# Token Monitor Ink

为 Poke3 等墨水屏设备提供 Token Monitor 只读 Android APK，最低 Android 10。APK 是本仓库唯一产品；账号管理、采集和跨设备汇总由另行运行的外部 Token Monitor 提供。

四页看板包含额度、会话、活动和明细，可切换日／月／累计用量。额度按服务商分组，使用环形额度与余额卡；会话采用分栏卡片，明细包含用量分块与分布环图。界面固定在可用视口内，以按钮分页和字段详情浏览，没有滚动或动画。黑白模式采用 16 阶灰度；彩色模式提供蓝、青、绿、琥珀、玫瑰、紫六种亮色重点色。

活动提供热力图、平滑趋势与每日／每月数值。曲线按真实日期定位，已有记录之间统一用平滑实线连接，不补成零；无记录不等于采集失败；零值观测保留，首尾无记录区域留白。总量、费用和分项可在“明细 → 用量”查看。

## 构建与连接

需要已有 JDK 17+、Android SDK platform 36 及匹配 build-tools，在仓库根目录执行：

```bash
export JAVA_HOME=/path/to/jdk
export ANDROID_HOME=/path/to/android-sdk
tools/build-eink-apk.sh debug
```

输出 `output/android/token-monitor-eink-debug.apk`。安装后填写设备可达的 Hub 地址及密钥；可按需另填已有归档服务的地址和独立密钥。本仓库不提供或维护 Hub、归档服务。

Hub 数据是采集快照，缺失值、来源过期和读取失败分别提示；费用为估算值。可选本机归档不代表跨设备全部历史，也不会重复累加进 Hub 总量。连接安全、数据契约、Release 签名与覆盖升级见 [Android 指南](docs/token-monitor-android.md)。

## 目录

```text
apps/android/          Android 工程
apps/android/ui/       包内界面与数据模型测试
assets/cats/           保留供以后复用的猫猫设计资产
tools/build-eink-apk.sh APK 构建入口
docs/                  Android 指南
```

## 开发检查

```bash
node --test apps/android/ui/test_*.mjs
node --check apps/android/ui/app.js
apps/android/gradlew -p apps/android :app:checkNativePolicy
```

当前版本已通过 Release 构建、原生连接策略与 39 项界面模型／曲线测试，以及 12 种视口下的 198 个模拟界面状态检查。已在 Poke3 覆盖安装并启动，原连接配置保留，额度与活动页在线显示，平滑实线趋势已在真机显示。具体验收范围见 Android 指南。

模拟尺寸检查不等于其他设备实测；长期耗电、物理残影和实体彩色墨水屏尚未测试。

许可证见 [LICENSE](LICENSE)。
