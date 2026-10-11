#!/usr/bin/env sh
# Free the container's RAM: stop it entirely (~0 MB). Installed apps and
# disabled components persist in the data volume; android-start.sh restores.
set -e
NAME=android15
docker stop "$NAME" 2>/dev/null || true
echo "android15 stopped (RAM freed). start again: scripts/android-start.sh"
