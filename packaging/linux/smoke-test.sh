#!/bin/sh
# Install the .deb with apt, check what it put in place, remove it, and
# check that all of it went (BH2). For a throwaway Debian/Ubuntu machine or a
# CI runner; it needs sudo.
#
#   sh packaging/linux/smoke-test.sh release/pocketdesk-<version>-linux-amd64.deb
set -u

deb=$(realpath "${1:?the .deb to test}")
failures=0
check() {
  if eval "$1"; then echo "ok    $2"; else echo "FAIL  $2"; failures=$((failures + 1)); fi
}

sudo DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends "$deb" >/dev/null
installed=$?
check '[ "$installed" -eq 0 ]' "apt installed it"
for bin in pocketdesk pocketdesk-app pocketdesk-tray; do
  check "[ -x /usr/bin/$bin ]" "installed /usr/bin/$bin"
done
check '/usr/bin/pocketdesk --version >/dev/null' "pocketdesk --version runs ($(/usr/bin/pocketdesk --version 2>&1))"
check 'grep -q "^ExecStart=/usr/bin/pocketdesk start" /usr/lib/systemd/user/pocketdesk.service' "the user unit starts /usr/bin/pocketdesk"
check '[ "$(systemctl --global is-enabled pocketdesk.service 2>/dev/null)" = enabled ]' "the user unit is on for every user"
check '[ -f /usr/share/applications/dev.pocketdesk.Pocketdesk.desktop ]' "the menu entry"
if command -v desktop-file-validate >/dev/null 2>&1; then
  check 'desktop-file-validate /usr/share/applications/dev.pocketdesk.Pocketdesk.desktop' "the menu entry is valid"
fi
check '[ -f /usr/share/icons/hicolor/512x512/apps/pocketdesk.png ]' "the icon"
check '[ -f /usr/share/man/man1/pocketdesk.1.gz ]' "the man page"
check '[ -f /usr/share/doc/pocketdesk/copyright ]' "the copyright file"

sudo DEBIAN_FRONTEND=noninteractive apt-get remove -y pocketdesk >/dev/null
removed=$?
check '[ "$removed" -eq 0 ]' "apt removed it"
check '[ ! -e /usr/bin/pocketdesk ] && [ ! -e /usr/bin/pocketdesk-app ]' "removed the programs"
check '[ ! -e /usr/lib/systemd/user/pocketdesk.service ]' "removed the user unit"
check '[ ! -e /etc/systemd/user/graphical-session.target.wants/pocketdesk.service ]' "no enable link left behind"
check '[ ! -e /usr/share/applications/dev.pocketdesk.Pocketdesk.desktop ]' "removed the menu entry"

if [ "$failures" -gt 0 ]; then
  echo "$failures check(s) failed" >&2
  exit 1
fi
echo "The .deb installs and removes cleanly."
