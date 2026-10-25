# Token Monitor Ink

为 Poke3 等墨水屏设备提供 Token Monitor 只读 Android APK，最低 Android 10。APK 是本仓库唯一产品；账号管理、采集和跨设备汇总由另行运行的外部 Token Monitor 提供。

四页看板包含额度、会话、活动和明细，可切换日／月／累计用量。界面采用编号导航、统一网格、大数字与细分隔线。额度卡片按账号实际信息量自动分配宽高，比较横向并排与上下堆叠排列；窗口多的卡片占更大区域，余额及重置券紧凑显示，数字和半圆刻度盘随可用空间适度放大。提供服务商目录与筛选。会话采用对齐列表，明细包含用量网格与比例分布。界面固定在可用视口内，支持按钮或左右轻扫分页；抬手后直接换页，元素不跟手移动，没有滚动或动画。黑白模式采用 16 阶灰度；彩色模式提供蓝、青、绿、琥珀、玫瑰、紫六种亮色重点色。

首次默认全屏，顶部图标可切换并记住选择。应用顶栏显示设备时间、电量／充电和网络链路；Hub 连接状态单独显示，未验证外网不会误判局域网断开。刷新、全屏、设置及翻页采用图标；日／月／累计并入导航行。服务商筛选使用带箭头和选中勾的分页下拉菜单，主名称在常用 Poke3 尺寸下约 18px 加粗，次要账号／套餐信息紧凑排列。

Poke3 原生刷新采用 16 阶灰度局部更新，每 6 次翻页或累计 30 次局部更新后全刷。设置 → 屏幕提供“清屏”；切换配色、恢复前台也请求全刷。其他未验证设备由系统管理刷新，不使用网页闪屏模拟硬件全刷。具体支持依据与验证边界见 Android 指南。

额度适配以 Token Monitor v0.78.0 的全部 30 家服务商为基线，支持百分比、金额、点数、次数、Token 池、消费额、不限额、重置券与分批到期余额。完整字段通过按钮详情读取，保留未知、零和过期的区别。逐家范围及旧 Hub 限制见 [Provider 覆盖表](docs/token-monitor-android.md#provider-额度覆盖)。

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
apps/android/gradlew -p apps/android :app:checkNativePolicy :app:checkDisplayRefresh :app:checkDeviceStatus
```

当前自适应卡片版通过 Release 构建与 72 项 JS 测试。8 种视口验证全部 121 项合成额度／余额／重置券完整可达，另覆盖零张／未知券、仅资料、重复账号和 33 个窗口。1.1.0 已在 Poke3 覆盖安装并冷启动，保留配置与缓存，确认额度页、自适应卡片、全屏和设备状态栏显示。布局保持无滚动和瞬时翻页；模拟检查与本次安装的具体边界见 Android 指南。

模拟尺寸检查不等于其他设备实测；长期耗电、物理残影和实体彩色墨水屏尚未测试。

许可证见 [LICENSE](LICENSE)。
