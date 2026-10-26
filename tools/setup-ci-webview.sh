#!/usr/bin/env bash
# Pin a standalone AOSP WebView for the API 29 CI emulator only.
set -euo pipefail

fail() { printf 'CI WebView: %s\n' "$*" >&2; exit 1; }
[[ ${CI:-} == true ]] || fail 'requires CI=true; never run against a personal device'

adb_cmd=(adb)
if [[ -n ${ANDROID_SERIAL:-} ]]; then
  adb_cmd+=(-s "$ANDROID_SERIAL")
else
  adb_cmd+=(-e)
fi
prop() { "${adb_cmd[@]}" shell getprop "$1" | tr -d '\r'; }
[[ $(prop ro.kernel.qemu) == 1 ]] || fail 'target must be an Android emulator'
case $(prop ro.build.type) in
  userdebug|eng) ;;
  *) fail 'target must be a userdebug or eng image' ;;
esac
[[ $(prop ro.build.version.sdk) == 29 ]] || fail 'target must be API 29'
[[ ,$(prop ro.product.cpu.abilist), == *,x86_64,* ]] || fail 'target must support x86_64'

# Official AOSP prebuilt, fetched as Gitiles base64 TEXT at an immutable commit.
# Manifest verified: com.android.webview, minSdk 26, versionCode 661308807,
# x86/x86_64 native libraries, no uses-static-library/Trichrome dependency.
# Upstream documents userdebug/eng provider signature exceptions:
# https://chromium.googlesource.com/chromium/src/+/HEAD/android_webview/docs/aosp-system-integration.md
revision=27ac2bdb9a1a4e55dc65bb6cdc752dbfdea3b9f4
version=128.0.6613.88
digest=deb1987501d40c4c326697be8fdd7733abdf2e0ab7fb6e028205b0ff786876e5
url="https://android.googlesource.com/platform/external/chromium-webview/+/$revision/$version/x86_64/webview.apk?format=TEXT"
tmp_dir=$(mktemp -d)
trap 'rm -rf -- "$tmp_dir"' EXIT
curl --fail --location --silent --show-error --connect-timeout 30 --max-time 300 \
  --retry 2 --retry-delay 2 --retry-max-time 900 \
  --output "$tmp_dir/webview.b64" "$url"
base64 --decode "$tmp_dir/webview.b64" > "$tmp_dir/webview.apk"
printf '%s  %s\n' "$digest" "$tmp_dir/webview.apk" | sha256sum --check --status
"${adb_cmd[@]}" install -r "$tmp_dir/webview.apk"
selection=$("${adb_cmd[@]}" shell cmd webviewupdate set-webview-implementation com.android.webview | tr -d '\r')
printf '%s\n' "$selection"
[[ $selection == 'Success' ]] || fail 'framework did not select the pinned provider'
state=$("${adb_cmd[@]}" shell dumpsys webviewupdate | tr -d '\r')
printf '%s\n' "$state"
[[ $state == *"Current WebView package (name, version): (com.android.webview, $version)"* ]] \
  || fail 'current WebView package/version differs from the pin'
