#!/bin/bash
# The GNOME window lend's live test (docs/development-setup.md §3a-2) in a
# throwaway Arch container: current GNOME Shell and mutter (the #643 research
# measured clicks arriving through a window's RemoteDesktop on mutter 51),
# headless on its own session bus, the Pocketdesk extension enabled, and
# windows that overlap. Run as root in the container; the Shell and the test
# run as `rig`. $1 is the lent window: wev or clicker.
set -eux
LENT=${1:-wev}

pacman -Syu --noconfirm --needed base-devel rust clang pkgconf pipewire libpipewire \
  wireplumber wayland mesa ffmpeg dbus gnome-shell mutter wev python-gobject gtk4 \
  procps-ng coreutils glib2 python-dbusmock

useradd -m rig
OWNER=$(stat -c %u:%g /src)
chown -R rig /src
RUN=/tmp/rig-run
install -d -m 700 -o rig "$RUN"

as_rig() { runuser -u rig -- env XDG_RUNTIME_DIR="$RUN" HOME=/home/rig \
  DBUS_SESSION_BUS_ADDRESS="unix:path=$RUN/bus" "$@"; }

as_rig bash -c 'cd /src && cargo test -p pocketdesk-server --test lend_window_gnome_live --no-run'

as_rig mkdir -p /home/rig/.local/share/gnome-shell/extensions
cp -r "/src/contrib/gnome-shell-extension/pocketdesk-windows@pocketdesk.dev" \
  /home/rig/.local/share/gnome-shell/extensions/
chown -R rig /home/rig/.local

# The Shell asks the system bus for logind and others at start; with none in
# the container its JS threw ("Could not connect: No such file or directory")
# and the Shell exited before the extension loaded.
install -d -m 755 /run/dbus
dbus-uuidgen --ensure
dbus-daemon --system --fork
# No systemd here, so no logind: the Shell's start asks it for the session and
# threw on the activation error. A mock, as GNOME's own CI runs.
nohup python3 -m dbusmock --system --template logind > /tmp/logind-mock.log 2>&1 &
for i in $(seq 20); do
  gdbus introspect --system --dest org.freedesktop.login1 --object-path /org/freedesktop/login1 >/dev/null 2>&1 && break
  sleep 0.5
done
as_rig dbus-daemon --session --address="unix:path=$RUN/bus" --fork
as_rig gsettings set org.gnome.shell disable-user-extensions false
as_rig gsettings set org.gnome.shell enabled-extensions "['pocketdesk-windows@pocketdesk.dev']"
as_rig bash -c 'nohup pipewire > /tmp/pipewire.log 2>&1 &'
sleep 1
as_rig bash -c 'nohup wireplumber > /tmp/wireplumber.log 2>&1 &'
as_rig bash -c 'LIBGL_ALWAYS_SOFTWARE=1 XDG_SESSION_TYPE=wayland XDG_CURRENT_DESKTOP=GNOME \
  nohup gnome-shell --headless --wayland --no-x11 --virtual-monitor 1280x720 > /tmp/gnome-shell.log 2>&1 &'
for i in $(seq 60); do [ -S "$RUN/wayland-0" ] && break; sleep 1; done
for i in $(seq 30); do
  as_rig gdbus call --session --dest dev.pocketdesk.Windows --object-path /dev/pocketdesk/Windows \
    --method dev.pocketdesk.Windows.List >/dev/null 2>&1 && break
  sleep 1
done
gnome-shell --version || true

W="WAYLAND_DISPLAY=wayland-0"
if [ "$LENT" = clicker ]; then
  as_rig bash -c "$W nohup python3 -u /src/scripts/rig_click_logger.py > /tmp/lent.log 2>&1 &"
else
  as_rig bash -c "$W nohup stdbuf -oL wev > /tmp/lent.log 2>&1 &"
fi
sleep 3
as_rig bash -c "$W nohup stdbuf -oL wev > /tmp/cover.log 2>&1 &"
sleep 2
as_rig bash -c "$W nohup stdbuf -oL wev > /tmp/third.log 2>&1 &"
sleep 3
chmod 644 /tmp/*.log || true

status=0
APPS=""
[ "$LENT" = clicker ] && APPS="PD_GNOME_RIG_LENT_APP=org.example.RigClicker PD_GNOME_RIG_COVER_APP=wev"
as_rig bash -c "cd /src && unset DBUS_SESSION_BUS_ADDRESS; \
  PD_GNOME_RIG_BUS=unix:path=$RUN/bus PD_GNOME_RIG_RUNTIME=$RUN PD_GNOME_RIG_SHELL=extension \
  PD_GNOME_RIG_LENT_LOG=/tmp/lent.log PD_GNOME_RIG_COVER_LOG=/tmp/cover.log $APPS \
  RUST_LOG=info,pocketdesk_core::streaming=debug \
  cargo test -p pocketdesk-server --test lend_window_gnome_live -- --ignored --nocapture --test-threads=1" || status=$?

echo "=== gnome-shell.log"; tail -n 60 /tmp/gnome-shell.log || true
echo "=== lent.log"; cat /tmp/lent.log || true
echo "=== cover.log"; tail -n 40 /tmp/cover.log || true
chown -R "$OWNER" /src || true
exit $status
