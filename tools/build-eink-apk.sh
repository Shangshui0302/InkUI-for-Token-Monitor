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
verify_args=(--variant "$variant")
if [[ "$variant" == release ]]; then
  if [[ -z "${TOKEN_MONITOR_SIGNING_FILE:-}" || ! -f "$TOKEN_MONITOR_SIGNING_FILE" ]]; then
    echo "Set TOKEN_MONITOR_SIGNING_FILE to a private release signing properties file." >&2
    exit 2
  fi
  gradle_args+=("-PtokenMonitorSigningFile=$TOKEN_MONITOR_SIGNING_FILE")
fi
if [[ -n "${TOKEN_MONITOR_RELEASE_TAG:-}" ]]; then
  if [[ "$variant" != release || -z "${TOKEN_MONITOR_PREVIOUS_VERSION_CODE:-}" || -z "${TOKEN_MONITOR_CERT_SHA256:-}" ]]; then
    echo "A formal release requires release variant, TOKEN_MONITOR_PREVIOUS_VERSION_CODE and TOKEN_MONITOR_CERT_SHA256." >&2
    exit 2
  fi
  verify_args+=(--tag "$TOKEN_MONITOR_RELEASE_TAG")
fi
if [[ -n "${TOKEN_MONITOR_PREVIOUS_VERSION_CODE:-}" ]]; then
  verify_args+=(--previous-code "$TOKEN_MONITOR_PREVIOUS_VERSION_CODE")
fi
if [[ -n "${TOKEN_MONITOR_CERT_SHA256:-}" ]]; then
  verify_args+=(--expected-cert-sha256 "$TOKEN_MONITOR_CERT_SHA256")
fi
for arg in "$@"; do
  if [[ "$arg" == -x* || "$arg" == --exclude-task* ]]; then
    echo "The APK delivery entry point requires its verification tasks; task exclusions are unsupported." >&2
    exit 2
  fi
done

task="assemble${variant^}"
# The Android shell bundles the same UI and has its own pinned Gradle wrapper.
"$repo_dir/apps/android/gradlew" "${gradle_args[@]}" ":app:$task" "$@"

destination="$repo_dir/output/android"
mkdir -p "$destination"
build_output="$repo_dir/apps/android/app/build/outputs/apk/$variant"
# AGP metadata supplies a candidate name only. The gate below independently
# reads versionName/code from the compiled manifest and checks current source.
read -r version code < <(python3 - "$build_output/output-metadata.json" <<'PY'
import json, re, sys
metadata = json.load(open(sys.argv[1]))
elements = metadata['elements']
if len(elements) != 1:
    sys.exit('Expected one universal APK')
element = elements[0]
version, code = element['versionName'], element['versionCode']
if not re.fullmatch(r'(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(-dev)?', version) or not isinstance(code, int):
    sys.exit('Invalid build version metadata')
print(version, code)
PY
)
artifact="$destination/inkui-$version-code$code-$variant.apk"
# Verify before replacing a previously delivered artifact.
staging_dir="$(mktemp -d "$destination/.verify-XXXXXXXX")"
trap 'rm -rf -- "$staging_dir"' EXIT
staged="$staging_dir/$(basename -- "$artifact")"
cp "$build_output/app-$variant.apk" "$staged"
"$repo_dir/tools/verify-eink-release.sh" "$staged" "${verify_args[@]}"
mv -- "$staged" "$artifact"
(cd "$destination" && sha256sum "$(basename -- "$artifact")" > "$(basename -- "$artifact").sha256")
echo "APK: $artifact"
