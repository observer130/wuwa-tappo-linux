#!/usr/bin/env bash
# Launch Wuwa Tappo. The binary sets the transparent-overlay environment
# defaults itself; this script adds the input group, the launcher-level
# single-instance lock and X11 backend preference used by the tested
# configuration. The binary also holds its own instance lock, so direct
# launches and other desktop entries cannot start a second copy.
set -euo pipefail

DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BIN="$DIR/bin/wuwatappo-linux"
if [[ ! -x "$BIN" ]]; then
  BIN="$DIR/src-tauri/target/release/wuwatappo-linux"
fi
if [[ ! -x "$BIN" ]]; then
  echo "wuwa-tappo: binary not found. Download a release, or build with 'npm run package'." >&2
  exit 1
fi

export GDK_BACKEND=x11
export WEBKIT_FORCE_COMPOSITING_MODE=1

if [[ -n "${XDG_RUNTIME_DIR:-}" ]]; then
  LOCK="$XDG_RUNTIME_DIR/wuwa-tappo-linux.lock"
else
  # /tmp is shared between users.
  LOCK="/tmp/wuwa-tappo-linux-${UID}.lock"
fi
exec 9>"$LOCK"
if ! flock -n 9; then
  echo "Wuwa Tappo is already running."
  exit 0
fi

cd "$DIR"
# Reading /dev/input/event* needs the input group. newgrp applies the group to
# this process without requiring a full logout when the user was just added.
# Use `id -nG` without a user argument: `id -nG "$USER"` queries /etc/group, so
# it reports input even when the current session has not inherited it yet.
if ! id -nG | tr ' ' '\n' | grep -qx input \
  && command -v newgrp >/dev/null 2>&1 \
  && getent group input >/dev/null 2>&1; then
  STARTED="${LOCK}.started"
  rm -f "$STARTED"
  if newgrp input -c "cd \"$DIR\" && : > \"$STARTED\" && GDK_BACKEND=x11 WEBKIT_FORCE_COMPOSITING_MODE=1 exec \"$BIN\" 9>&-"; then
    rm -f "$STARTED"
    exit 0
  else
    status=$?
    # A marker means newgrp reached the app; a non-zero status is the app's
    # own exit status. Do not start a second instance without input in that
    # case, or the lock stays held after a crash or forced quit.
    if [[ -f "$STARTED" ]]; then
      rm -f "$STARTED"
      exit "$status"
    fi
  fi
  rm -f "$STARTED"
  echo "wuwa-tappo: 无法通过 newgrp 应用 input 组，将以当前权限启动。" >&2
  echo "如果键鼠计数不可用，请重新登录，或在终端执行 'newgrp input' 后再运行。" >&2
fi

# Keep this shell alive as the lock owner and close the lock fd in the app.
# Exec'ing the binary leaks fd 9 into WebKit's child processes, which can keep
# the flock held after the app exits and make the next launch report "already
# running".
"$BIN" 9>&-
