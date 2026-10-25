# Token Monitor Android

独立的 Java Activity + WebView APK，构建时直接从 `apps/eink` 编入同一套界面。APK 不包含 Hub、服务商客户端或本机归档数据库；在应用的连接设置中填写局域网 Hub origin，按需另填本机累计会话归档网关 origin。

要求 JDK 17+、Android SDK platform 36 和相应 build tools。默认使用 Gradle 8.14.3、AGP 8.11.0，无 Kotlin / Rust / AndroidX 运行时依赖。

```sh
cd apps/android
ANDROID_HOME=/path/to/Android/Sdk ./gradlew :app:assembleDebug :app:checkNativePolicy
```

输出 `app/build/outputs/apk/debug/app-debug.apk`，调试包名为 `dev.tokenmonitor.eink.debug`，仅调试包开启 WebView 调试。

发布包使用独立私有签名配置；将 `keystore.properties.example` 复制到仓库外，填入自己的密钥文件和密码后运行：

```sh
./gradlew :app:assembleRelease -PtokenMonitorSigningFile=/absolute/path/to/private/signing.properties
```

发布包名 `dev.tokenmonitor.eink`；没有私有签名配置时发布任务明确失败。仓库不提供签名密钥。

界面仅允许虚拟 HTTPS origin 的白名单资源；三个只读 API 由原生固定 GET 请求转发，不接受重定向、代理、页面提供的请求头或 Cookie。Hub 与可选归档密钥作为一个 AES-GCM 加密记录存入应用私有 no-backup 目录，密钥由 Android Keystore 管理。JavaScript 只能读取地址、配置版本和密钥是否已保存，不能读取已保存密钥。更改 origin 且没有显式提交新密钥时，会清除对应旧密钥。配置 revision 改变后，旧配置发起的请求结果丢弃为通用错误。

本轮已在 Poke3 验证安装、完整数据读取、前后台停读、返回、加密连接保存及冷启动／断线恢复。构建、连接方式与验收范围见 [完整说明](../../docs/token-monitor-android.md)；模拟尺寸不代表其他实体设备验收，长期耗电和残影尚未测试。
