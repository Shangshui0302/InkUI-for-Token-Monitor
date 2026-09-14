# Android APK compatibility review

Review date: 2026-09-14

Current Android architecture: the APK is an independent P-256 peer and native dashboard, not a WebView shell. The optional WebView is only a compatibility viewer for a desktop node. On the tested Poke3, the native dashboard remains usable from persisted authenticated snapshots while desktop HTTP port 18788 is unreachable.

## Approved icon update

The page header, PWA and APK now use the approved half-face pixel black cat with
yellow-green eyes. Density-specific PNGs use nearest-neighbor resizing from
`web/app-icon-master.png`. Android adaptive icons inset the artwork by 20% so
launcher masks preserve the eyes; the PWA icon declares `any` rather than
claiming maskable coverage. The static cache version is v22; both 192px and 512px
PWA icons are embedded by the node and listed for offline caching.

This APK rebuild reused the previously prepared Android 35 build tools
and platform, with the available Nix-store JDK 21 and `javac --release 8`.
The declared JDK 17 development shell could not be realized because the binary
cache timed out; APK assembly and signature verification succeeded locally.

## Scope and evidence

- APK: `com.codexmonitor.mobile` 0.1.0, `minSdkVersion=29`, `targetSdkVersion=29`, compiled with Android platform 35 and Java 8 bytecode.
- Device proof: BOOX Poke3, Android API 29, ARM64, Google Android WebView 108.0.5359.128.
- The APK contains Java/Dex and resources only, with no native ABI libraries.
- Local artifact uses an APK Signature Scheme v3 test key. It is not a store or production signing setup.

## Compatibility matrix

| Surface | Result | Evidence boundary |
| --- | --- | --- |
| Poke3 / Android 10 | Passed | Upgrade install, launcher start, native node/dashboard, optional WebView state, frameless immersive mode, system-bar toggles and cold-start persistence were exercised on the device. |
| Native peer node on Poke3 | Earlier protocol runtime pass; current authenticated-code transcript not device-tested | Earlier real-Wi-Fi testing covered discovery, signed P-256 pairing and no-code persisted-trust reconnect. The current HMAC-SHA256 code-proof/transcript update is structurally covered by the desktop/Android build tests only; repeat the LAN/device test before claiming runtime compatibility. |
| Native node UI on Poke3 | Passed | The Chinese black-cat header, node and one-time-code cards, compact input, two-column nearby/paired lists, Advanced dialog and WebView return path were exercised at the device's 1448×1072 landscape resolution. The full native page fits without scrolling. |
| Native monitoring dashboard | Passed | Poke3 received and persisted P-256-authenticated snapshots over TCP 8787, rendered native overview/quota/activity/session tabs, and retained the view while desktop HTTP 18788 remained blocked. Light and dark palettes were exercised; e-ink is a separate light-only mode. |
| Legacy launcher icon | Passed | Five density-specific PNG mipmaps are packaged and the Onyx launcher displays the generated black-cat icon. |
| Android 8+ adaptive/round icon | Structurally passed | `mipmap-anydpi-v26` adaptive and round resources resolve through the packaged resource table; only Poke3 was visually inspected. |
| Android 11–15 | Compatible by API surface, not device-tested | Deprecated system-UI flags remain available to a target-29 app, but modern system-bar behavior was not exercised on these releases. |
| WebView 108 | Passed | Poke3 loaded the live UI and Android-only bridge settings after the PNG route was fixed. |
| Chrome/WebView 85 baseline | Compatibility accommodations, not a full runtime pass | `replaceChildren` is polyfilled. CSS `accent-color` falls back to the native checkbox color; a complete run on version 85 remains outstanding. |
| PWA icons | Structurally passed | Both 192px and 512px PNGs are served and listed in the manifest and service-worker assets; install presentation varies by browser/launcher. |
| Offline reopen over LAN HTTP | Not guaranteed | Service workers require a secure context. Localhost USB-forwarding tests do not establish offline support for ordinary LAN HTTP. HTTPS must cover both the page and node API; an already open page can still retain its last snapshot. |

## Findings

### Historical compatibility fixes

The following entries record the earlier WebView/PWA compatibility review. They are historical implementation notes, not a description of the current Android primary UI.

1. The APK had no declared launcher icon. It now packages legacy PNG, round and adaptive resources generated from one master image.
2. The direct `aapt2` build did not compile app resources. The build now compiles the resource directory and links the compiled resource archive as the primary resource set.
3. The monitor server did not expose the new `/app-icon.png`, which produced a broken header image on Poke3. The PNG is now embedded, served as `image/png`, and covered by the static-asset unit test.
4. An unreachable saved node address previously trapped users in a failed WebView with no independent settings entry. Root-page Back now opens a native recovery menu; main-document network/HTTP errors and a 30-second load timeout open an editable native address form. Subresource errors are ignored, and stale WebView callbacks are guarded.
5. The manifest now includes a 192px PNG alongside the 512px icon. The node serves the new resource, and the static-asset test checks its MIME type, PNG signature and dimensions.
6. Deployment instructions and connection settings now distinguish LAN HTTP online access from HTTPS-capable offline reopening. The application does not bypass browser secure-context or mixed-content requirements.
7. At that review stage, Android advertised `peer-sync-v1` and carried `networkId` through discovery and pairing. The current Android node uses the P-256 authenticated snapshot protocol documented in [`protocol-v2.md`](protocol-v2.md); do not treat this earlier protocol label as the current Android dashboard contract.
8. `PairAccepted` now echoes the initiator ID required by the protocol. The desktop inbound path also marks an already-connected Android node as connected when its first UDP announcement arrives after the TCP session.
9. The native node page now shares the PWA's Chinese copy, black-cat identity, monochrome cards and compact hierarchy. Already-paired peers are no longer duplicated in the nearby-node column; manual addressing and the full public key moved to an Advanced dialog.

### Remaining low-risk boundaries

The recovery paths were exercised on Poke3: root Back menu, refused connection,
main-document HTTP 404, and a server delaying its response by 45 seconds. The
last case reaches the native recovery form after 30 seconds. A successful HTML
page with an HTTP 404 image stays in the WebView. The saved address was restored
to the working monitor after the checks.

1. `SYSTEM_UI_FLAG_*` and `onBackPressed()` are deprecated on newer Android APIs. They are appropriate for the current Android 10 target, but a future target-SDK update should add `WindowInsetsController` and `OnBackInvokedDispatcher` paths before claiming current Android platform integration.
2. Android 13+ themed monochrome launcher icons are not supplied. Those systems fall back to the colored adaptive icon.
3. The JavaScript bridge is available to the configured WebView page and its frames. It exposes only system-bar booleans and the local address form—no token, file, shell or arbitrary network capability—but the configured HTTPS endpoint remains a user trust decision.
4. `usesCleartextTraffic=true` is necessarily broad at manifest level because the LAN host is entered at runtime. Native URL validation and request interception restrict actual cleartext WebView loads to loopback, private and link-local addresses or `.local` names.
5. Onyx may apply its own freeze/snowflake lifecycle and always owns its floating navigation overlays. Those behaviors are outside the APK's system-bar controls.

## Before public distribution

- Retest on at least one Android 12 device and one Android 14/15 device.
- Raise the target SDK and add modern insets/back-navigation code when distribution requirements demand it.
- Add an Android 13 monochrome icon if themed-icon integration is desired.
- Replace the local test key with a separately managed release-signing process.
