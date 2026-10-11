#!/usr/bin/env sh
# Trim the running Android's memory: drop caches, kill background processes,
# trim the biggest residents. Repeatable after heavy app sessions.
ADB=${LITEBOX_ADB:-/opt/android-sdk/platform-tools/adb}
S="-s 127.0.0.1:5555"
"$ADB" $S shell pm trim-caches 999999999 2>/dev/null
"$ADB" $S shell am kill-all 2>/dev/null
for p in $($ADB $S shell pidof com.android.systemui com.android.launcher3 com.android.systemui 2>/dev/null); do
    "$ADB" $S shell am send-trim-memory "$p" COMPLETE 2>/dev/null || true
done
docker stats --no-stream --format "android15: {{.MemUsage}}" 2>/dev/null || true
