# Android WebView shell

This is a small, native Java APK wrapper for the existing Codex Monitor web UI. It has no cross-platform framework and contains no monitor business logic. On first launch it shows a local address form; the URL is kept in app-private preferences. The Android-only section in the web UI's Display settings can reopen that form later. No token, cookie, or authentication flow is copied into the APK.

The activity has no native title bar. Once the monitor UI is open, status and navigation bars are hidden by default with Android immersive mode. The two Android-only Display settings change those choices immediately and persist them in app-private preferences. Edge swipes can reveal system bars temporarily; Onyx system overlays remain controlled by the device.

At the web UI's root page, Android Back opens a native menu with Change address, Retry, Exit, and Cancel. A failed main-document request, HTTP status 400 or higher, or a 30-second main-document loading timeout opens the native address recovery form with an error explanation, editable address, retry, and exit actions. Subresource failures do not replace the page with recovery UI. Old WebView callbacks are ignored after the WebView is replaced or the active main-document URL changes.

The PWA, page header and Android legacy/round/adaptive launcher resources use the same generated black-cat master image. See [`../../docs/android-compatibility.md`](../../docs/android-compatibility.md) for the tested matrix and remaining platform boundaries.

## Build

The build uses Nixpkgs' pinned Android SDK composition, JDK 17, and the Android command-line tools directly. It does not use Gradle or `sdkmanager`. The build script runs `aapt2`, `javac`, `d8`, `zipalign`, `keytool`, and `apksigner`; after those Nix tools are available, APK assembly itself is local and does not contact Maven or Google.

The Android SDK is under Google's license. Review and accept that license explicitly before building. `NIXPKGS_ACCEPT_ANDROID_SDK_LICENSE=1` is Nixpkgs' one-invocation license-acceptance switch; do not set it unless you accept the terms. `NIXPKGS_ALLOW_UNFREE=1` permits the unfree SDK package. Nix may need to fetch pinned SDK artifacts into the Nix store if they are not already cached, but this does not use `sdkmanager`.

From this directory, after accepting the Android SDK license:

```sh
NIXPKGS_ALLOW_UNFREE=1 \
NIXPKGS_ACCEPT_ANDROID_SDK_LICENSE=1 \
nix develop --impure --command ./build.sh
```

The installable test-signed APK is written to `.build/CodexMonitor-test.apk`. Generated files and the test key stay under the ignored `.build/` directory.

## Install and uninstall

Connect and authorize the BOOX Poke3 over USB debugging, then run from this directory:

```sh
adb install -r .build/CodexMonitor-test.apk
```

Uninstall with:

```sh
adb uninstall com.codexmonitor.mobile
```

The application requires Android 10 (API 29) or newer. Enter a node URL such as `https://monitor.example/` or an explicitly local `http://192.168.1.20:8080/`. Plain HTTP is accepted only for loopback, RFC1918/link-local IPv4, IPv6 loopback/link-local/site-local/unique-local, or `.local` hostnames. Other cleartext URLs and cleartext subresource requests are blocked by the WebView client. HTTPS URLs remain available normally.

Android's network-security configuration cannot express an arbitrary, user-entered LAN node host as a narrow static exception. Consequently, the APK opts into cleartext at the app manifest level so explicitly local HTTP endpoints can work; the native URL validator and WebView request gate constrain actual HTTP navigation and resources to local hosts. No other cleartext client or feature is present in the app. Use HTTPS whenever available.

## Signing boundary

The build script creates a local debug/test RSA key only if `.build/test-key.p12` does not already exist, then signs the APK with it. The keystore is ignored by Git and must not be shared or used for distribution. This is a locally installable test artifact, not a production release. A release requires a separately managed signing key and an independently reviewed signing process; this project does not create or store one.
