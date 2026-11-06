#!/bin/bash
# The window lend's live test (server/tests/lend_window_live.rs) against a
# Hyprland nested in a headless Sway, inside a throwaway Arch container.
# Run as root in the container; the compositors and the test run as `rig`.
set -eux

pacman -Syu --noconfirm --needed base-devel rust clang pkgconf pipewire libpipewire \
  wayland mesa ffmpeg dbus sway hyprland wev grim procps-ng coreutils

useradd -m rig
chown -R rig /src
RUN=/tmp/rig-run
install -d -m 700 -o rig "$RUN"

cat > /tmp/sway.conf <<'EOF'
output HEADLESS-1 resolution 1920x1080
EOF
cat > /tmp/hypr.conf <<'EOF'
monitor = , 1600x900, 0x0, 1
misc {
  disable_hyprland_logo = true
  disable_splash_rendering = true
}
EOF
chmod 644 /tmp/sway.conf /tmp/hypr.conf

as_rig() { runuser -u rig -- env XDG_RUNTIME_DIR="$RUN" HOME=/home/rig CARGO_HOME=/home/rig/.cargo "$@"; }

as_rig bash -c 'cd /src && cargo test -p pocketdesk-server --test lend_window_live --no-run'

as_rig bash -c "WLR_BACKENDS=headless WLR_RENDERER=pixman WLR_LIBINPUT_NO_DEVICES=1 \
  WAYLAND_DISPLAY= nohup sway -c /tmp/sway.conf > /tmp/sway.log 2>&1 &"
for i in $(seq 30); do ls "$RUN"/wayland-* >/dev/null 2>&1 && break; sleep 1; done
OUTER=$(basename "$(ls "$RUN"/wayland-* | grep -v lock | head -1)")
echo "outer: $OUTER"

as_rig bash -c "WAYLAND_DISPLAY=$OUTER HYPRLAND_NO_SD_NOTIFY=1 \
  nohup Hyprland -c /tmp/hypr.conf > /tmp/hyprland.log 2>&1 &"
for i in $(seq 40); do ls "$RUN"/hypr/*/.socket.sock >/dev/null 2>&1 && break; sleep 1; done
ls -la "$RUN" "$RUN"/hypr || true
SIG=$(basename "$(dirname "$(ls "$RUN"/hypr/*/.socket.sock | head -1)")")
INNER=$(ls "$RUN" | grep '^wayland-' | grep -v lock | grep -vx "$OUTER" | head -1)
echo "hyprland: $SIG on $INNER"

status=0
as_rig bash -c "cd /src && unset WAYLAND_DISPLAY HYPRLAND_INSTANCE_SIGNATURE SWAYSOCK; \
  PD_NESTED_WAYLAND=$INNER PD_NESTED_HYPRLAND=$SIG RUST_LOG=info \
  cargo test -p pocketdesk-server --test lend_window_live -- --ignored --nocapture --test-threads=1" || status=$?

echo "=== sway.log"; tail -n 40 /tmp/sway.log || true
echo "=== hyprland.log"; tail -n 120 /tmp/hyprland.log || true
for f in "$RUN"/hypr/*/hyprland.log; do echo "=== $f"; tail -n 120 "$f"; done 2>/dev/null || true
exit $status
