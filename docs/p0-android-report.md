# P0 Android 真机验收报告

日期：2026-09-15

设备：Poke3，ADB serial `82CCF4BF`

测试包：`dev.nekodex.workbench.p0`

## 结论

最小 Android WebView 壳已在目标真机安装并运行；环境与本地页面骨架达到继续做 P0 集成的条件。本文只把有命令或当前代码证据的项目标为“已验证”。真实 App Server 闭环、长时间运行和部分视觉/输入体验仍未验证，不能据此宣称 P0 全部通过。

## 已验证

| 项目 | 结果 | 证据 |
|---|---|---|
| Android 版本/API | 已验证：Android 10，API 29 | `adb -s 82CCF4BF shell getprop ro.build.version.release` → `10`；`ro.build.version.sdk` → `29` |
| ABI | 已验证：设备支持 `arm64-v8a`（同时列出 armeabi-v7a/armeabi） | `getprop ro.product.cpu.abilist` |
| WebView | 已验证：系统 WebView `com.google.android.webview` 108.0.5359.128 | `dumpsys webviewupdate`，Current/Preferred WebView package 均为该版本 |
| 安装、冷启动后的前台进程 | 已验证：包已安装，`MainActivity` resumed，进程存活 | `pm path` 返回 `/data/app/.../base.apk`；`pidof` 返回 `27173`；`dumpsys activity activities` 的 `mResumedActivity` 为 `dev.nekodex.workbench.p0/.MainActivity` |
| 本地资源与 IPC 文案 | 已验证到代码/壳实现：页面声明本地相对 CSS/JS 资源；`p0_environment` 成功时显示“本地资源 · IPC 正常” | `apps/ui/index.html`、`apps/ui/app.js`、`apps/shell/src-tauri/src/lib.rs`；本次未伪造服务端数据 |
| 导航 | 已验证：真机点击“活动”后仅活动面板显示，当前项状态同步变化 | 真机交互与临时验收截图；处理器位于 `apps/ui/app.js` |
| 软键盘与文本 | 部分验证：点击输入框后 `mInputShown=true`，ADB ASCII 文本 `Poke3-test` 可见 | `dumpsys input_method` 与真机临时截图；尚未验证中文 IME 输入 |
| 审批弹层 | 已验证演示交互：真机打开弹层后“拒绝/允许”按钮可见 | 真机临时截图；这仍是 UI 探针，未连接真实 App Server 审批回调 |
| WebSocket | 已验证：真机页面连接 `ws://127.0.0.1:18789`，经 `adb reverse` 到本机最小 echo 服务后显示“WebSocket 正常” | `tools/p0-websocket-echo.mjs` 与真机临时截图；只证明 Poke3 WebView 的基础 WebSocket 兼容性，不证明设备组网络链路 |
| 桌面专用能力隔离 | 已验证：Poke3 点击“运行桌面 App Server 验证”后通过 IPC 返回“Poke3 是控制端，不在本机运行 Codex App Server” | 最新 APK 真机键盘焦点/点击与临时截图；证明桌面依赖未被误用于 Android，不证明远程调用已实现 |
| 冷启动 Activity 耗时 | 已记录一次：`am start -W` 的 `TotalTime=1658ms` | 此值不是首屏内容绘制时间；约 5 秒时的中间截图仍为空白，因此不能声称首屏 2 秒目标通过 |
| 内存快照 | 已记录两次 PSS：约 76,344 KiB 与 99,294 KiB | `adb -s 82CCF4BF shell dumpsys meminfo dev.nekodex.workbench.p0`；均低于 200MB 目标，但单点值不能证明长期稳定性 |

## 已记录的修复（实现/验收背景）

- 图标曾有两条通道崩溃：资源生成/加载路径与 launcher foreground 的 RGBA 处理不一致；已按 RGBA 资源修复。当前安装包可启动，且本次截取的 logcat 未出现该包的 `FATAL EXCEPTION`。这证明当前启动没有复现崩溃，不证明所有图标密度/系统 launcher 组合均已覆盖。
- 已修复 edge-to-edge 下的顶部遮挡；页面声明 `viewport-fit=cover`，当前 Activity 可正常 resumed。未把没有逐像素截图对比的观察升级为“所有异形屏/旋转场景通过”。
- 已把 Android 模板紫色状态栏/导航栏改为电子墨水浅色系统栏并启用深色图标；最新 APK 真机截图已确认紫色状态栏消失。
- ADB 触摸滑动在本次环境没有稳定驱动 WebView；键盘焦点导航可把页面下方控制区滚入视野。Poke3 的 Onyx 悬浮导航按钮仍覆盖页面边缘，这是设备级浮层，不属于应用 DOM。

## 未验证 / 不应夸大

- 未验证真实中文字符输入、长文本滚动和提交链路；只确认软键盘弹起与 ASCII 文本注入。
- 未验证残影、电子墨水刷新质量、耗电、前台空闲 CPU 低于 2%。
- 未验证后台 30 分钟后恢复、断网缓存、旋转/恢复。
- 未验证缓存首屏 2 秒目标；`am start -W` 只记录 Activity 启动，不能替代内容绘制测量。
- 未验证真实 Codex App Server 的创建→发送→审批→停止闭环，也未验证跨设备网络状态更新。
- 未验证 Android 内存长期稳定性；单次 PSS 不能替代压力/后台测试。

## 可复核命令摘要

```sh
adb -s 82CCF4BF shell getprop ro.build.version.release
adb -s 82CCF4BF shell getprop ro.build.version.sdk
adb -s 82CCF4BF shell getprop ro.product.cpu.abilist
adb -s 82CCF4BF shell dumpsys webviewupdate
adb -s 82CCF4BF shell pm path dev.nekodex.workbench.p0
adb -s 82CCF4BF shell pidof dev.nekodex.workbench.p0
adb -s 82CCF4BF shell dumpsys activity activities
adb -s 82CCF4BF shell dumpsys input_method
adb -s 82CCF4BF shell dumpsys meminfo dev.nekodex.workbench.p0
adb -s 82CCF4BF logcat -d -t 300
```

本次只启动/读取测试包状态，未清除旧应用数据、未触碰其他应用、未执行系统部署。
