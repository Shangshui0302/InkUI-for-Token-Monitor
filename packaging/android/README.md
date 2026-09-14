# Android native peer node

This is a native Java peer node for Android 10 and newer. A foreground service owns a persistent AndroidKeyStore P-256 identity, UDP multicast/broadcast discovery, the TCP 8787 peer listener, one-time pairing codes, and the trusted-peer store independently of the Activity and WebView lifetimes. It does not collect Codex data or hold desktop Codex credentials.

The Activity owns a persisted, authenticated monitoring dashboard with Overview,
Quota, Activity, and Sessions tabs. Data arrives as P-256-signed snapshots over
TCP 8787, so the native dashboard does not depend on a desktop HTTP port. It has
light and dark palettes plus a separate high-contrast e-ink mode; dark mode is
not offered while e-ink mode is selected.

The launcher opens a compact Chinese e-ink-oriented native node page using the same black-cat visual language as the web UI. It shows this device's identity, current one-time code, nearby nodes, paired nodes, and connection state in one landscape screen; duplicate paired entries are collapsed. Enter the other node's current eight-character code to pair a discovered node. The P-256 key and manual `host:port` fallback live under Advanced settings. Refreshing the local code invalidates the previous code; a successful first pairing consumes it. Saved public-key trust is used for later reconnects without another code. The device can be renamed locally.

The displayed code is never sent in `AgentHello`: Android and desktop exchange role-separated HMAC-SHA256 proofs over the signed identity/key/network/challenge transcript. A node saves new trust only after validating the signed acceptance and the peer's code-possession proof. This short-code HMAC is not a PAKE; a captured proof permits offline guessing. See [`../../docs/protocol-v2.md`](../../docs/protocol-v2.md) for the exact transcript and security boundary.

The native Activity also provides a persistent overview/quota/activity/sessions dashboard populated by P-256-authenticated snapshots over TCP 8787. It does not need desktop HTTP 18788 to render an already received snapshot. The optional WebView is only a compatibility viewer for an existing desktop node; its saved URL stays in app-private preferences, and no token, cookie, or authentication flow is copied into the native node service.

The activity has no native title bar. Once the monitor UI is open, status and navigation bars are hidden by default with Android immersive mode. The two Android-only Display settings change those choices immediately and persist them in app-private preferences. Edge swipes can reveal system bars temporarily; Onyx system overlays remain controlled by the device.

In the optional WebView, Android Back opens a native menu with Change address, Retry, Exit, and Cancel. A failed main-document request, HTTP status 400 or higher, or a 30-second main-document loading timeout opens the native address recovery form with an error explanation, editable address, retry, and exit actions. Subresource failures do not replace the page with recovery UI. Old WebView callbacks are ignored after the WebView is replaced or the active main-document URL changes.

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

The build also runs the pure-Java pairing-code concurrency/refresh test. The installable test-signed APK is written to `.build/CodexMonitor-test.apk`. Generated files and the test key stay under the ignored `.build/` directory.

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
