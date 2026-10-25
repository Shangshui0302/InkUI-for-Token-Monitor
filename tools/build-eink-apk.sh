#!/usr/bin/env bash
set -euo pipefail

repo_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
variant="${1:-debug}"
if [[ "$variant" != debug && "$variant" != release ]]; then
  echo "Usage: tools/build-eink-apk.sh [debug|release] [Gradle options...]" >&2
  exit 2
fi
if [[ $# -gt 0 ]]; then shift; fi

if [[ -z "${ANDROID_HOME:-${ANDROID_SDK_ROOT:-}}" ]]; then
  echo "Set ANDROID_HOME to an existing Android SDK with platform 36 and build tools." >&2
  exit 2
fi
export ANDROID_HOME="${ANDROID_HOME:-$ANDROID_SDK_ROOT}"
export ANDROID_SDK_ROOT="$ANDROID_HOME"
if [[ -n "${JAVA_HOME:-}" ]]; then export PATH="$JAVA_HOME/bin:$PATH"; fi
if ! command -v java >/dev/null 2>&1; then
  echo "Set JAVA_HOME to an existing JDK 17 or newer." >&2
  exit 2
fi

gradle_args=(--no-daemon --console=plain -p "$repo_dir/apps/android")
if [[ "$variant" == release ]]; then
  if [[ -z "${TOKEN_MONITOR_SIGNING_FILE:-}" || ! -f "$TOKEN_MONITOR_SIGNING_FILE" ]]; then
    echo "Set TOKEN_MONITOR_SIGNING_FILE to a private release signing properties file." >&2
    exit 2
  fi
  gradle_args+=("-PtokenMonitorSigningFile=$TOKEN_MONITOR_SIGNING_FILE")
fi

task="assemble${variant^}"
# The Android shell bundles the same UI and has its own pinned Gradle wrapper.
"$repo_dir/apps/android/gradlew" "${gradle_args[@]}" ":app:$task" "$@"

destination="$repo_dir/output/android"
mkdir -p "$destination"
artifact="$destination/inkui-$variant.apk"
cp "$repo_dir/apps/android/app/build/outputs/apk/$variant/app-$variant.apk" "$artifact"
(cd "$destination" && sha256sum "inkui-$variant.apk" > "inkui-$variant.apk.sha256")
echo "APK: $artifact"
