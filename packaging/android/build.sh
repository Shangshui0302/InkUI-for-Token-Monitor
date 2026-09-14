#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")"

SDK_ROOT="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-}}"
COMPILE_SDK="${ANDROID_COMPILE_SDK:-35}"
BUILD_TOOLS="${ANDROID_BUILD_TOOLS:-35.0.0}"
ANDROID_JAR="$SDK_ROOT/platforms/android-$COMPILE_SDK/android.jar"
TOOLS="$SDK_ROOT/build-tools/$BUILD_TOOLS"
OUTPUT_DIR="$PWD/.build"
CLASSES_DIR="$OUTPUT_DIR/classes"
DEX_DIR="$OUTPUT_DIR/dex"
CLASSES_JAR="$OUTPUT_DIR/classes.jar"
COMPILED_RESOURCES="$OUTPUT_DIR/resources.zip"
TEST_CLASSES_DIR="$OUTPUT_DIR/test-classes"

if [[ -z "$SDK_ROOT" || ! -f "$ANDROID_JAR" ]]; then
  echo "Android SDK platform android-$COMPILE_SDK not found; enter this directory's Nix dev shell first." >&2
  exit 1
fi
for tool in aapt2 d8 zipalign apksigner; do
  if [[ ! -x "$TOOLS/$tool" ]]; then
    echo "Required Android build tool missing: $TOOLS/$tool" >&2
    exit 1
  fi
done

mkdir -p "$CLASSES_DIR" "$DEX_DIR" "$TEST_CLASSES_DIR"
rm -f "$OUTPUT_DIR/base.apk" "$OUTPUT_DIR/unsigned.apk" \
  "$OUTPUT_DIR/aligned.apk" "$OUTPUT_DIR/CodexMonitor-test.apk" \
  "$CLASSES_JAR" "$COMPILED_RESOURCES" "$DEX_DIR/classes.dex"

"$TOOLS/aapt2" compile \
  --dir app/src/main/res \
  -o "$COMPILED_RESOURCES"

"$TOOLS/aapt2" link \
  -I "$ANDROID_JAR" \
  --manifest app/src/main/AndroidManifest.xml \
  --min-sdk-version 29 \
  --target-sdk-version 29 \
  --version-code 1 \
  --version-name 0.1.0 \
  -o "$OUTPUT_DIR/base.apk" \
  "$COMPILED_RESOURCES"

mapfile -t JAVA_SOURCES < <(find app/src/main/java -type f -name '*.java' -print | sort)
if [[ ${#JAVA_SOURCES[@]} -eq 0 ]]; then
  echo "No Java sources found under app/src/main/java." >&2
  exit 1
fi
"${JAVA_HOME:+$JAVA_HOME/bin/}javac" \
  --release 8 \
  -classpath "$ANDROID_JAR" \
  -d "$CLASSES_DIR" \
  "${JAVA_SOURCES[@]}"

"${JAVA_HOME:+$JAVA_HOME/bin/}javac" --release 8 \
  -d "$TEST_CLASSES_DIR" \
  app/src/main/java/com/codexmonitor/mobile/PairingCode.java \
  app/src/test/java/com/codexmonitor/mobile/PairingCodeLogicTest.java
"${JAVA_HOME:+$JAVA_HOME/bin/}java" -cp "$TEST_CLASSES_DIR" \
  com.codexmonitor.mobile.PairingCodeLogicTest

"${JAVA_HOME:+$JAVA_HOME/bin/}jar" --create --file "$CLASSES_JAR" -C "$CLASSES_DIR" .
"$TOOLS/d8" --min-api 29 --lib "$ANDROID_JAR" --output "$DEX_DIR" "$CLASSES_JAR"
cp "$OUTPUT_DIR/base.apk" "$OUTPUT_DIR/unsigned.apk"
(cd "$DEX_DIR" && zip -q "$OUTPUT_DIR/unsigned.apk" classes.dex)
"$TOOLS/zipalign" -f -p 4 "$OUTPUT_DIR/unsigned.apk" "$OUTPUT_DIR/aligned.apk"

if [[ ! -f "$OUTPUT_DIR/test-key.p12" ]]; then
  keytool -genkeypair \
    -keystore "$OUTPUT_DIR/test-key.p12" \
    -storetype PKCS12 \
    -storepass android \
    -keypass android \
    -alias androiddebugkey \
    -keyalg RSA \
    -keysize 2048 \
    -validity 10000 \
    -dname "CN=Codex Monitor Local Test,O=Codex Monitor,C=US"
fi

"$TOOLS/apksigner" sign \
  --ks "$OUTPUT_DIR/test-key.p12" \
  --ks-key-alias androiddebugkey \
  --ks-pass pass:android \
  --key-pass pass:android \
  --out "$OUTPUT_DIR/CodexMonitor-test.apk" \
  "$OUTPUT_DIR/aligned.apk"
"$TOOLS/apksigner" verify --verbose "$OUTPUT_DIR/CodexMonitor-test.apk"
echo "Built $OUTPUT_DIR/CodexMonitor-test.apk"
