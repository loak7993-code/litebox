#!/usr/bin/env sh
# Start the Android 15 container (redroid) and wait for adb.
# RAM profile: ~1GB capped, headless, software rendering.
# Data (installed apps, disabled components) persists in /var/lib/redroid15-data.
set -e
NAME=android15
if docker ps --format '{{.Names}}' | grep -qx "$NAME"; then
    echo "android15 already running"
else
    docker start "$NAME" 2>/dev/null || docker run -d \
        --privileged --name "$NAME" \
        -v /var/lib/redroid15-data:/data \
        -p 5555:5555 \
        --memory 1536m --memory-swap 2048m \
        redroid/redroid:15.0.0-latest \
        androidboot.use_memfd=1
fi
ADB=${LITEBOX_ADB:-/opt/android-sdk/platform-tools/adb}
"$ADB" start-server >/dev/null 2>&1
i=0
until "$ADB" shell getprop sys.boot_completed 2>/dev/null | grep -q 1; do
    i=$((i + 1))
    [ "$i" -gt 60 ] && { echo "boot timeout"; exit 1; }
    sleep 2
done
"$ADB" connect 127.0.0.1:5555 >/dev/null 2>&1
echo "android15 ready: adb -s 127.0.0.1:5555 (mem: $(docker stats --no-stream --format '{{.MemUsage}}' $NAME))"
