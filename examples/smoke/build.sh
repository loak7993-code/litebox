#!/usr/bin/env sh
# Build the smoke-test APK with the real Android toolchain:
# javac -> d8 -> aapt2 -> zip -> apksigner.
# Requires: JDK 11+, Android SDK build-tools + platforms (ANDROID_HOME or
# /opt/android-sdk), and the apksigner debug keystore at ~/.android.
set -e
cd "$(dirname "$0")"

BT=${ANDROID_HOME:-/opt/android-sdk}/build-tools/$(ls ${ANDROID_HOME:-/opt/android-sdk}/build-tools | sort -V | tail -1)
PLAT=${ANDROID_HOME:-/opt/android-sdk}/platforms/$(ls ${ANDROID_HOME:-/opt/android-sdk}/platforms | grep -E '^android-[0-9]+$' | sort -V | tail -1)
AJ=$PLAT/android.jar

rm -rf build
mkdir -p build/classes build/dex

echo "[1/5] javac"
javac --release 11 -cp "$AJ" -d build/classes java/com/litebox/smoke/*.java

echo "[2/5] d8"
"$BT/d8" --release --lib "$AJ" --min-api 24 --output build/dex $(find build/classes -name '*.class')

echo "[3/5] aapt2 link"
"$BT/aapt2" link -I "$AJ" --manifest AndroidManifest.xml -o build/smoke-unsigned.apk --auto-add-overlay

echo "[4/5] package dex"
cp build/dex/classes.dex ./classes.dex
zip -q -u build/smoke-unsigned.apk classes.dex
rm classes.dex

echo "[5/5] sign"
"$BT/zipalign" -f 4 build/smoke-unsigned.apk build/smoke-aligned.apk
"$BT/apksigner" sign \
  --ks "$HOME/.android/debug.keystore" \
  --ks-pass pass:android \
  --ks-key-alias androiddebugkey \
  --out build/smoke.apk \
  build/smoke-aligned.apk
"$BT/apksigner" verify build/smoke.apk

echo "built: $(pwd)/build/smoke.apk"
