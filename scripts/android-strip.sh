#!/usr/bin/env sh
# Strip the android15 container to vanilla + basic apps. Idempotent.
# Everything is reversible: adb shell pm enable --user 0 <package>
ADB=${LITEBOX_ADB:-/opt/android-sdk/platform-tools/adb}
S="-s 127.0.0.1:5555"
BLOAT="com.android.nfc com.android.se com.android.uwb.resources
com.android.printservice.recommendation com.android.printspooler com.android.bips
com.android.egg com.android.dreams.basic com.android.dreams.phototable
com.android.wallpaper.livepicker com.android.wallpaperbackup com.android.wallpapercropper
com.android.musicfx com.android.quicksearchbox com.android.traceur
com.android.devicediagnostics com.android.devicelockcontroller com.android.managedprovisioning
com.android.hotspot2.osulogin com.android.adservices.api
com.android.ondevicepersonalization.services com.android.federatedcompute.services
com.android.htmlviewer com.android.mtp com.android.DeviceAsWebcam com.android.cameraextensions
com.android.cts.ctsshim com.android.cts.priv.ctsshim com.android.dynsystem
com.android.healthconnect.controller com.android.health.connect.backuprestore
com.android.rkpdapp com.android.sdksandbox org.chromium.webview_shell
com.android.phone com.android.server.telecom com.android.mms.service
com.android.smspush com.android.simappdialog
com.android.bluetooth com.android.inputmethod.latin"
for p in $BLOAT; do
    state=$("$ADB" $S shell pm list packages -d 2>/dev/null | grep -x "package:$p")
    if [ -z "$state" ] && "$ADB" $S shell pm list packages 2>/dev/null | grep -qx "package:$p"; then
        "$ADB" $S shell pm disable-user --user 0 "$p" >/dev/null 2>&1 && echo "disabled: $p"
    fi
done
"$ADB" $S shell pm trim-caches 999999999 >/dev/null 2>&1 || true
echo "done. kept: launcher, systemui, settings, calendar, camera, clock, contacts, files, gallery, music, webview(lib)"
