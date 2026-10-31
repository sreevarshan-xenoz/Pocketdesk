#!/usr/bin/env bash
#
# Pocketdesk Installer
# Downloads and installs Pocketdesk (the pocketdesk server) from GitHub Releases.
#
# Usage: ./install.sh [VERSION] [OPTIONS]
#
# Arguments:
#   VERSION    Version tag (e.g., v0.1.0). Defaults to 'latest'.
#
# Options:
#   --yes, -y                  Non-interactive mode; accept all defaults
#   --dry-run                  Show what would be done without making changes
#   --verbose                  Show detailed debug output
#   --force                    Force reinstall even if current version matches
#   --no-service               Skip systemd service installation
#   --no-config                Skip config file creation
#   --no-docs                  Skip documentation installation
#   --no-man                   Skip man page installation
#   --prefix PATH              Install prefix (default: /usr)
#   --check-updates            Check if installed version is up to date
#   --list-versions            List all available releases
#   --status                   Show current installation status
#   --rollback                 Roll back to the previous version
#   --uninstall                Remove an existing installation
#   -h, --help                 Show this help message
#
# Examples:
#   ./install.sh                    # Install latest release
#   ./install.sh v0.1.0             # Install specific version
#   ./install.sh --yes              # Non-interactive install
#   ./install.sh --dry-run          # Preview installation
#   ./install.sh --prefix /opt      # Install to /opt
#   ./install.sh --check-updates    # Check for updates
#   ./install.sh --list-versions    # List available versions
#   ./install.sh --status           # Show installation info
#   ./install.sh --rollback         # Roll back to previous version
#   ./install.sh --uninstall        # Remove installation

set -euo pipefail

# ─── Configuration ───────────────────────────────────────────────────────────

REPO_OWNER="sreevarshan-xenoz"
REPO_NAME="Pocketdesk"
BINARY_NAME="pocketdesk"
INSTALL_PREFIX="/usr"
SERVICE_FILE=""
CONFIG_DIR="$HOME/.config/${BINARY_NAME}"
CONFIG_FILE=""
STATE_FILE=""
BACKUP_DIR=""

# ─── Flags (set by argument parsing) ────────────────────────────────────────

DRY_RUN=false
VERBOSE=false
FORCE=false
NO_SERVICE=false
NO_CONFIG=false
NO_DOCS=false
NO_MAN=false
NON_INTERACTIVE=false
UNINSTALL=false
CHECK_UPDATES=false
LIST_VERSIONS=false
SHOW_STATUS=false
ROLLBACK=false
VERSION="latest"

# ─── Color output ────────────────────────────────────────────────────────────

if [ -t 1 ]; then
  RED='\033[0;31m'
  GREEN='\033[0;32m'
  YELLOW='\033[0;33m'
  BLUE='\033[0;34m'
  CYAN='\033[0;36m'
  BOLD='\033[1m'
  DIM='\033[2m'
  RESET='\033[0m'
else
  RED='' GREEN='' YELLOW='' BLUE='' CYAN='' BOLD='' DIM='' RESET=''
fi

info()    { echo -e "${BLUE}[info]${RESET} $*"; }
ok()      { echo -e "${GREEN}[ok]${RESET} $*"; }
warn()    { echo -e "${YELLOW}[warn]${RESET} $*"; }
error()   { echo -e "${RED}[error]${RESET} $*" >&2; }
debug()   { if [ "$VERBOSE" = true ]; then echo -e "${DIM}[debug]${RESET} $*"; fi; }
fatal()   { error "$@"; exit 1; }
section() { echo -e "\n${BOLD}═══ $* ═══${RESET}"; }

# ─── Resolve paths after prefix is known ────────────────────────────────────

# The server captures the desktop (Wayland/X11) and talks to PipeWire, so it
# runs as a *systemd user* unit tied to the graphical session — a system unit
# can never see those. The target user is whoever owns the install: root's
# account when the script runs under sudo, else the invoking user.
resolve_paths() {
  SERVICE_USER="${SUDO_USER:-$USER}"
  SERVICE_UID="$(id -u "$SERVICE_USER")"
  SERVICE_GROUP="$(id -gn "$SERVICE_USER")"
  SERVICE_USER_HOME="$(getent passwd "$SERVICE_UID" | cut -d: -f6)"
  SERVICE_FILE="${SERVICE_USER_HOME}/.config/systemd/user/${BINARY_NAME}.service"
  CONFIG_FILE="${CONFIG_DIR}/config.toml"
  STATE_FILE="${CONFIG_DIR}/.install-state"
  BACKUP_DIR="${CONFIG_DIR}/.backups"
}

# Run 'systemctl --user' as the service user from a root/sudo context: the
# user manager needs XDG_RUNTIME_DIR + the session bus to be reachable.
user_systemctl() {
  sudo -u "$SERVICE_USER" env \
    "XDG_RUNTIME_DIR=/run/user/${SERVICE_UID}" \
    "DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/${SERVICE_UID}/bus" \
    systemctl --user "$@"
}

# Is the target user's session manager actually up? Enabling/starting a user
# unit requires it; the unit file itself can always be installed.
user_session_live() {
  [ -S "/run/user/${SERVICE_UID}/bus" ] && user_systemctl is-system-running &>/dev/null
}

# ─── Argument parsing ────────────────────────────────────────────────────────

parse_args() {
  for arg in "$@"; do
    case "$arg" in
      --yes|-y)               NON_INTERACTIVE=true ;;
      --dry-run)              DRY_RUN=true ;;
      --verbose)              VERBOSE=true ;;
      --force)                FORCE=true ;;
      --no-service)           NO_SERVICE=true ;;
      --no-config)            NO_CONFIG=true ;;
      --no-docs)              NO_DOCS=true ;;
      --no-man)               NO_MAN=true ;;
      --uninstall)            UNINSTALL=true ;;
      --check-updates)        CHECK_UPDATES=true ;;
      --list-versions)        LIST_VERSIONS=true ;;
      --status)               SHOW_STATUS=true ;;
      --rollback)             ROLLBACK=true ;;
      -h|--help)              show_help; exit 0 ;;
      --prefix)
        shift
        INSTALL_PREFIX="${1:?--prefix requires a path argument}"
        ;;
      --prefix=*)
        INSTALL_PREFIX="${arg#--prefix=}"
        ;;
      -v*)
        # Accept -v0.1.0 as version (but not --verbose which is handled above)
        VERSION="${arg#-v}"
        ;;
      v[0-9]*)
        VERSION="$arg"
        ;;
      [0-9]*)
        VERSION="v${arg}"
        ;;
      -*)
        fatal "Unknown option: ${arg}. Run '$0 --help' for usage."
        ;;
      *)
        VERSION="$arg"
        ;;
    esac
  done
}

show_help() {
  cat <<'EOF'
Pocketdesk Installer — Installation and management tool

USAGE
    install.sh [VERSION] [OPTIONS]

ARGUMENTS
    VERSION       Version tag (e.g., v0.1.0). Defaults to 'latest'.

OPTIONS
    -y, --yes             Non-interactive; accept all defaults
    --dry-run             Preview actions without making changes
    --verbose             Show detailed debug output
    --force               Force reinstall even if same version installed
    --no-service          Skip systemd service installation
    --no-config           Skip config file creation
    --no-docs             Skip documentation installation
    --no-man              Skip man page installation
    --prefix PATH         Install prefix (default: /usr)
    --check-updates       Check if installed version is up to date
    --list-versions       List all available releases
    --status              Show current installation status
    --rollback            Roll back to the previous version
    --uninstall           Remove an existing installation
    -h, --help            Show this help message

EXAMPLES
    install.sh                        Install latest release
    install.sh v0.1.0                 Install specific version
    install.sh --yes                  Non-interactive install
    install.sh --dry-run              Preview all actions
    install.sh --prefix /opt          Install to /opt instead of /usr
    install.sh --no-service           Skip systemd service setup
    install.sh --check-updates        Check for available updates
    install.sh --list-versions        Show all available versions
    install.sh --status               Show installed version and info
    install.sh --rollback             Revert to previous version
    install.sh --uninstall            Remove installation

ENVIRONMENT
    HTTP_PROXY, HTTPS_PROXY   Proxy settings passed to curl automatically
    NO_COLOR                  Disable colored output when set

EXIT CODES
    0    Success
    1    Fatal error (network, permissions, missing dependencies)
    2    Invalid arguments
    3    Version not found
    4    Checksum verification failed
    5    Rollback failed

For more information, see: https://github.com/sreevarshan-xenoz/Pocketdesk
EOF
}

# ─── Cleanup on exit / interrupt ─────────────────────────────────────────────

TMPDIR=""
cleanup() {
  if [ -n "$TMPDIR" ] && [ -d "$TMPDIR" ]; then
    debug "Cleaning up temporary directory: ${TMPDIR}"
    rm -rf "$TMPDIR"
  fi
}
trap cleanup EXIT INT TERM

# ─── State management ────────────────────────────────────────────────────────

save_state() {
  local version="$1"
  local prev_version="$2"
  [ "$DRY_RUN" = true ] && return
  mkdir -p "$(dirname "$STATE_FILE")"
  cat > "$STATE_FILE" <<EOF
version=${version}
previous_version=${prev_version}
install_date=$(date -u +%Y-%m-%dT%H:%M:%SZ)
prefix=${INSTALL_PREFIX}
EOF
  debug "State saved to ${STATE_FILE}"
}

load_state() {
  if [ -f "$STATE_FILE" ]; then
    # shellcheck disable=SC1090
    source "$STATE_FILE"
    debug "Loaded state: version=${version:-unknown} from ${STATE_FILE}"
    return 0
  fi
  return 1
}

get_previous_version() {
  if [ -f "$STATE_FILE" ]; then
    grep '^previous_version=' "$STATE_FILE" 2>/dev/null | cut -d= -f2 || true
  fi
}

# ─── Status command ──────────────────────────────────────────────────────────

cmd_status() {
  resolve_paths
  section "Installation Status"

  if [ -f "${INSTALL_PREFIX}/bin/${BINARY_NAME}" ]; then
    local bin_path="${INSTALL_PREFIX}/bin/${BINARY_NAME}"
    local ver
    ver=$("$bin_path" --version 2>/dev/null || echo "unknown")
    ok "Binary installed: ${bin_path} (${ver})"
  else
    error "Binary not installed."
  fi

  if [ -f "$SERVICE_FILE" ]; then
    ok "Service file (user unit): ${SERVICE_FILE}"
    if command -v systemctl &>/dev/null && user_session_live; then
      if user_systemctl is-active --quiet "$BINARY_NAME" 2>/dev/null; then
        ok "Service status: active"
      elif user_systemctl is-enabled --quiet "$BINARY_NAME" 2>/dev/null; then
        warn "Service status: enabled (not running)"
      else
        warn "Service status: disabled"
      fi
    fi
  else
    warn "Service file: not installed"
  fi

  if [ -f "$CONFIG_FILE" ]; then
    ok "Config: ${CONFIG_FILE}"
  else
    warn "Config: not found at ${CONFIG_FILE}"
  fi

  if [ -f "$STATE_FILE" ]; then
    ok "Install state: ${STATE_FILE}"
    if load_state; then
      echo "  Installed version:  ${version:-unknown}"
      echo "  Previous version:   ${previous_version:-none}"
      echo "  Install date:       ${install_date:-unknown}"
      echo "  Install prefix:     ${prefix:-unknown}"
    fi
  else
    warn "Install state: not tracked"
  fi

  # Check latest release
  echo ""
  info "Checking for updates..."
  local latest
  latest=$(fetch_latest_version 2>/dev/null) || latest="unknown"
  if [ "$latest" != "unknown" ] && [ -f "${INSTALL_PREFIX}/bin/${BINARY_NAME}" ]; then
    local current
    current=$("${INSTALL_PREFIX}/bin/${BINARY_NAME}" --version 2>/dev/null || echo "unknown")
    if [ "$current" = "$latest" ]; then
      ok "Latest version: ${latest} — you are up to date."
    else
      warn "Latest version: ${latest} — you have ${current}."
      warn "Run '$0 --yes' to upgrade."
    fi
  fi
}

# ─── List versions command ──────────────────────────────────────────────────

cmd_list_versions() {
  resolve_paths
  section "Available Releases"

  local releases
  releases=$(curl -fsSL --max-time 15 \
    -H "Accept: application/vnd.github+json" \
    "https://api.github.com/repos/${REPO_OWNER}/${REPO_NAME}/releases?per_page=20" 2>/dev/null) || \
    fatal "Cannot reach GitHub API. Check your internet connection."

  # Parse tag_name and published_at
  echo -e "${BOLD}  Version${RESET}        ${BOLD}Published${RESET}"
  echo "  ───────────────────────────────"

  echo "$releases" | grep -E '"tag_name"|"published_at"' | \
    paste - - | \
    sed 's/.*"tag_name": *"\([^"]*\)".*"published_at": *"\([^"]*\)".*/  \1   \2/' | \
    while IFS= read -r line; do
      echo "$line"
    done

  echo ""
  info "For older versions, visit: https://github.com/${REPO_OWNER}/${REPO_NAME}/releases"
}

# ─── Check updates command ──────────────────────────────────────────────────

cmd_check_updates() {
  resolve_paths

  if [ ! -f "${INSTALL_PREFIX}/bin/${BINARY_NAME}" ]; then
    fatal "Pocketdesk is not installed. Run '$0' to install."
  fi

  local current
  current=$("${INSTALL_PREFIX}/bin/${BINARY_NAME}" --version 2>/dev/null || echo "unknown")
  info "Installed version: ${current}"

  local latest
  latest=$(fetch_latest_version) || fatal "Cannot check for updates."

  if [ "$current" = "$latest" ]; then
    ok "You are running the latest version (${current})."
  else
    warn "Update available: ${current} → ${latest}"
    echo ""
    echo "  To upgrade, run:  $0 --yes"
    echo "  To rollback:      $0 --rollback"
  fi
}

# ─── Rollback command ───────────────────────────────────────────────────────

cmd_rollback() {
  resolve_paths

  local prev_version
  prev_version=$(get_previous_version)

  if [ -z "$prev_version" ]; then
    fatal "No previous version to rollback to. State file may be missing."
  fi

  section "Rollback to ${prev_version}"
  warn "This will reinstall Pocketdesk ${prev_version}."
  echo ""

  if [ "$NON_INTERACTIVE" != true ]; then
    read -rp "Continue? [y/N] " confirm
    if [[ ! "$confirm" =~ ^[Yy]$ ]]; then
      echo "Aborted."
      exit 0
    fi
  fi

  # Run full install with the previous version
  VERSION="$prev_version"
  FORCE=true
  info "Rolling back to ${prev_version}..."
  cmd_install
}

# ─── Helper: fetch latest version ────────────────────────────────────────────

fetch_latest_version() {
  local response
  response=$(curl -fsSL --max-time 15 \
    -H "Accept: application/vnd.github+json" \
    "https://api.github.com/repos/${REPO_OWNER}/${REPO_NAME}/releases/latest" 2>/dev/null) || \
    return 1

  echo "$response" | grep -o '"tag_name": *"[^"]*"' | head -1 | \
    sed 's/"tag_name": *"//;s/"//'
}

# ─── Uninstall mode ──────────────────────────────────────────────────────────

cmd_uninstall() {
  resolve_paths
  section "Removing Pocketdesk"

  if [ "$DRY_RUN" = true ]; then
    info "DRY RUN — showing what would be removed."
  fi

  # Stop and disable the user service
  if command -v systemctl &>/dev/null && user_session_live; then
    if user_systemctl is-active --quiet "$BINARY_NAME" 2>/dev/null; then
      info "Stopping ${BINARY_NAME} user service..."
      [ "$DRY_RUN" = true ] || user_systemctl stop "$BINARY_NAME"
    fi
    if user_systemctl is-enabled --quiet "$BINARY_NAME" 2>/dev/null; then
      info "Disabling ${BINARY_NAME} user service..."
      [ "$DRY_RUN" = true ] || user_systemctl disable "$BINARY_NAME"
    fi
  fi

  # Remove any leftover system unit from older installs
  local legacy_unit="/etc/systemd/system/${BINARY_NAME}.service"
  if [ -f "$legacy_unit" ]; then
    info "Removing legacy system unit ${legacy_unit}..."
    [ "$DRY_RUN" = true ] || sudo systemctl disable --now "$BINARY_NAME" 2>/dev/null || true
    [ "$DRY_RUN" = true ] || sudo rm -f "$legacy_unit"
    [ "$DRY_RUN" = true ] || sudo systemctl daemon-reload
  fi

  # Remove binary
  if [ -f "${INSTALL_PREFIX}/bin/${BINARY_NAME}" ]; then
    info "Removing binary ${INSTALL_PREFIX}/bin/${BINARY_NAME}..."
    [ "$DRY_RUN" = true ] || sudo rm -f "${INSTALL_PREFIX}/bin/${BINARY_NAME}" "${INSTALL_PREFIX}/bin/pocketdesk-app" "${INSTALL_PREFIX}/bin/pocketdesk-tray"
  fi

  # Remove service file
  if [ -f "$SERVICE_FILE" ]; then
    info "Removing service file ${SERVICE_FILE}..."
    [ "$DRY_RUN" = true ] || rm -f "$SERVICE_FILE"
    [ "$DRY_RUN" = true ] || { command -v systemctl &>/dev/null && user_session_live && user_systemctl daemon-reload; } || true
  fi

  # Remove docs
  local doc_dir="${INSTALL_PREFIX}/share/doc/${BINARY_NAME}"
  if [ -d "$doc_dir" ]; then
    info "Removing documentation ${doc_dir}..."
    [ "$DRY_RUN" = true ] || sudo rm -rf "$doc_dir"
  fi

  # Remove man page
  local man_page="${INSTALL_PREFIX}/share/man/man1/${BINARY_NAME}.1"
  if [ -f "$man_page" ]; then
    info "Removing man page ${man_page}..."
    [ "$DRY_RUN" = true ] || sudo rm -f "$man_page"
    [ "$DRY_RUN" = true ] || mandb -q 2>/dev/null || true
  fi

  # Remove state file
  if [ -f "$STATE_FILE" ]; then
    [ "$DRY_RUN" = true ] || rm -f "$STATE_FILE"
  fi

  # Remove backups
  if [ -d "$BACKUP_DIR" ]; then
    warn "Backup directory found: ${BACKUP_DIR}"
    if [ "$NON_INTERACTIVE" = true ]; then
      info "Non-interactive mode — removing backups."
      [ "$DRY_RUN" = true ] || rm -rf "$BACKUP_DIR"
    else
      read -rp "Remove backup configs (${BACKUP_DIR})? [y/N] " confirm
      if [[ "$confirm" =~ ^[Yy]$ ]]; then
        [ "$DRY_RUN" = true ] || rm -rf "$BACKUP_DIR"
      fi
    fi
  fi

  # Keep user config, just warn
  if [ -d "$CONFIG_DIR" ]; then
    warn "User config kept at ${CONFIG_DIR} (not removed)."
    warn "Remove manually with: rm -rf ${CONFIG_DIR}"
  fi

  echo ""
  ok "Uninstallation complete."
  if [ "$DRY_RUN" = true ]; then
    echo ""
    warn "DRY RUN — no changes were made."
  fi
}

# ─── System Detection ───────────────────────────────────────────────────────

# Global variables populated by detect_* functions
OS_ID=""
OS_NAME=""
OS_VERSION=""
OS_VERSION_ID=""
PKG_MANAGER=""
SESSION_TYPE=""
COMPOSITOR=""
PORTAL_BACKEND=""
INIT_SYSTEM=""
HAS_PIPEWIRE=false
HAS_TAILSCALE=false

detect_os() {
  debug "Detecting operating system..."

  if [ -f /etc/os-release ]; then
    # shellcheck disable=SC1091
    . /etc/os-release
    OS_ID="${ID:-unknown}"
    OS_NAME="${PRETTY_NAME:-unknown}"
    OS_VERSION_ID="${VERSION_ID:-}"
    OS_VERSION="${VERSION:-${VERSION_ID:-unknown}}"
  elif [ -f /etc/lsb-release ]; then
    # shellcheck disable=SC1091
    . /etc/lsb-release
    OS_ID="${DISTRIB_ID:-unknown}"
    OS_NAME="${DISTRIB_DESCRIPTION:-unknown}"
    OS_VERSION_ID="${DISTRIB_RELEASE:-}"
    OS_VERSION="${DISTRIB_DESCRIPTION:-unknown}"
  else
    OS_ID="unknown"
    OS_NAME="$(uname -o 2>/dev/null || echo unknown)"
    OS_VERSION="unknown"
  fi

  # Determine package manager
  if command -v dnf &>/dev/null; then
    PKG_MANAGER="dnf"
  elif command -v yum &>/dev/null; then
    PKG_MANAGER="yum"
  elif command -v zypper &>/dev/null; then
    PKG_MANAGER="zypper"
  elif command -v pacman &>/dev/null; then
    PKG_MANAGER="pacman"
  elif command -v apt-get &>/dev/null; then
    PKG_MANAGER="apt"
  elif command -v apk &>/dev/null; then
    PKG_MANAGER="apk"
  elif command -v nix-env &>/dev/null; then
    PKG_MANAGER="nix"
  else
    PKG_MANAGER="unknown"
  fi

  debug "  OS: ${OS_ID} (${OS_NAME})"
  debug "  Version: ${OS_VERSION}"
  debug "  Package manager: ${PKG_MANAGER}"
}

detect_compositor() {
  debug "Detecting display compositor..."

  # Priority 1: XDG_SESSION_TYPE (set by systemd-logind)
  if [ -n "${XDG_SESSION_TYPE:-}" ]; then
    SESSION_TYPE="$XDG_SESSION_TYPE"
  # Priority 2: Check WAYLAND_DISPLAY first (can't have X11 if Wayland is active)
  elif [ -n "${WAYLAND_DISPLAY:-}" ]; then
    SESSION_TYPE="wayland"
  # Priority 3: Check DISPLAY (X11 or XWayland)
  elif [ -n "${DISPLAY:-}" ]; then
    SESSION_TYPE="x11"
  # Priority 4: Check for running compositor processes
  elif pgrep -x "Hyprland\|sway\|river\|wayfire" &>/dev/null; then
    SESSION_TYPE="wayland"
  elif pgrep -x "Xorg\|Xwayland\|X" &>/dev/null; then
    SESSION_TYPE="x11"
  else
    SESSION_TYPE="unknown"
  fi

  # Detect specific compositor
  COMPOSITOR="unknown"

  if [ "$SESSION_TYPE" = "wayland" ]; then
    # Check XDG_CURRENT_DESKTOP first
    local desktop="${XDG_CURRENT_DESKTOP:-}"
    case "$desktop" in
      *Hyprland*)    COMPOSITOR="hyprland" ;;
      *GNOME*)       COMPOSITOR="gnome" ;;
      *KDE*)         COMPOSITOR="kde" ;;
      *sway*)        COMPOSITOR="sway" ;;
      *wlroots*)     COMPOSITOR="wlroots" ;;
      *river*)       COMPOSITOR="river" ;;
      *Wayfire*)     COMPOSITOR="wayfire" ;;
      *phosh*)       COMPOSITOR="phosh" ;;
      *Unity*)       COMPOSITOR="unity" ;;
    esac

    # If XDG_CURRENT_DESKTOP didn't help, check running processes
    if [ "$COMPOSITOR" = "unknown" ]; then
      if pgrep -x "Hyprland" &>/dev/null; then
        COMPOSITOR="hyprland"
      elif pgrep -x "sway" &>/dev/null; then
        COMPOSITOR="sway"
      elif pgrep -x "gnome-shell" &>/dev/null; then
        COMPOSITOR="gnome"
      elif pgrep -x "kwin_wayland" &>/dev/null; then
        COMPOSITOR="kde"
      elif pgrep -x "river" &>/dev/null; then
        COMPOSITOR="river"
      elif pgrep -x "wayfire" &>/dev/null; then
        COMPOSITOR="wayfire"
      fi
    fi

    # Detect portal backend
    if pgrep -x "xdg-desktop-portal-hyprland" &>/dev/null; then
      PORTAL_BACKEND="hyprland"
    elif pgrep -x "xdg-desktop-portal-gnome" &>/dev/null; then
      PORTAL_BACKEND="gnome"
    elif pgrep -x "xdg-desktop-portal-kde" &>/dev/null; then
      PORTAL_BACKEND="kde"
    elif pgrep -x "xdg-desktop-portal-wlr" &>/dev/null; then
      PORTAL_BACKEND="wlr"
    elif pgrep -x "xdg-desktop-portal-lxqt" &>/dev/null; then
      PORTAL_BACKEND="lxqt"
    elif pgrep -x "xdg-desktop-portal" &>/dev/null; then
      PORTAL_BACKEND="generic"
    else
      PORTAL_BACKEND="none"
    fi

  elif [ "$SESSION_TYPE" = "x11" ]; then
    COMPOSITOR="x11"
    PORTAL_BACKEND="none"
  fi

  debug "  Session type: ${SESSION_TYPE}"
  debug "  Compositor: ${COMPOSITOR}"
  debug "  Portal backend: ${PORTAL_BACKEND}"
}

detect_init_system() {
  debug "Detecting init system..."

  if command -v systemctl &>/dev/null && [ -d /run/systemd/system ]; then
    INIT_SYSTEM="systemd"
  elif command -v rc-status &>/dev/null; then
    INIT_SYSTEM="openrc"
  elif command -v sv &>/dev/null; then
    INIT_SYSTEM="runit"
  elif command -s 6 &>/dev/null 2>&1 || command -v s6-svscan &>/dev/null; then
    INIT_SYSTEM="s6"
  elif [ -d /etc/init.d ] && [ ! -d /run/systemd/system ]; then
    INIT_SYSTEM="sysvinit"
  else
    INIT_SYSTEM="unknown"
  fi

  debug "  Init system: ${INIT_SYSTEM}"
}

detect_system_capabilities() {
  debug "Detecting system capabilities..."

  # PipeWire
  if command -v pipewire &>/dev/null || pkg-config --exists libpipewire-0.3 2>/dev/null; then
    HAS_PIPEWIRE=true
  fi

  # Tailscale
  if command -v tailscale &>/dev/null && command -v tailscaled &>/dev/null; then
    HAS_TAILSCALE=true
  fi

  debug "  PipeWire: ${HAS_PIPEWIRE}"
  debug "  Tailscale: ${HAS_TAILSCALE}"
}

print_system_report() {
  section "System Detection Report"

  echo -e "  ${BOLD}Operating System:${RESET}  ${OS_NAME}"
  echo -e "  ${BOLD}Distribution:${RESET}      ${OS_ID} ${OS_VERSION_ID:-}"
  echo -e "  ${BOLD}Package Manager:${RESET}   ${PKG_MANAGER}"
  echo -e "  ${BOLD}Init System:${RESET}       ${INIT_SYSTEM}"
  echo ""
  echo -e "  ${BOLD}Display Server:${RESET}    ${SESSION_TYPE}"
  echo -e "  ${BOLD}Compositor:${RESET}        ${COMPOSITOR}"
  echo -e "  ${BOLD}Portal Backend:${RESET}    ${PORTAL_BACKEND}"
  echo ""
  echo -e "  ${BOLD}Architecture:${RESET}      $(uname -m)"
  echo -e "  ${BOLD}Kernel:${RESET}            $(uname -r)"
  echo ""
  echo -e "  ${BOLD}PipeWire:${RESET}          $(_status_icon "$HAS_PIPEWIRE" "Available" "Not detected")"
  echo -e "  ${BOLD}Tailscale:${RESET}         $(_status_icon "$HAS_TAILSCALE" "Detected" "Not detected")"
  echo ""

  # Provide environment-specific recommendations
  _print_recommendations
}

_status_icon() {
  if [ "$1" = true ]; then
    echo -e "${GREEN}✅ $2${RESET}"
  else
    echo -e "${YELLOW}⚠️  $3${RESET}"
  fi
}

_print_recommendations() {
  local missing=()

  # Recommendations based on detected environment
  if [ "$SESSION_TYPE" = "wayland" ] && [ "$PORTAL_BACKEND" = "none" ]; then
    missing+=("xdg-desktop-portal (required for screen capture on Wayland)")
  fi

  if [ "$HAS_PIPEWIRE" = false ] && [ "$SESSION_TYPE" = "wayland" ]; then
    missing+=("pipewire (required for screen capture on Wayland)")
  fi

  if [ "$HAS_TAILSCALE" = false ]; then
    missing+=("tailscale (required for remote connectivity)")
  fi

  if [ "$SESSION_TYPE" = "wayland" ]; then
    case "$COMPOSITOR" in
      hyprland)
        if [ "$PORTAL_BACKEND" != "hyprland" ]; then
          missing+=("xdg-desktop-portal-hyprland (portal backend for Hyprland)")
        fi
        ;;
      gnome)
        if [ "$PORTAL_BACKEND" != "gnome" ]; then
          missing+=("xdg-desktop-portal-gnome (portal backend for GNOME)")
        fi
        ;;
      kde)
        if [ "$PORTAL_BACKEND" != "kde" ]; then
          missing+=("xdg-desktop-portal-kde (portal backend for KDE)")
        fi
        ;;
      sway|wlroots)
        if [ "$PORTAL_BACKEND" != "wlr" ]; then
          missing+=("xdg-desktop-portal-wlr (portal backend for wlroots)")
        fi
        ;;
    esac
  fi

  if [ ${#missing[@]} -gt 0 ]; then
    echo -e "  ${BOLD}${YELLOW}⚠ Recommendations:${RESET}"
    for item in "${missing[@]}"; do
      echo -e "    ${YELLOW}•${RESET} $item"
    done
    echo ""
  else
    echo -e "  ${GREEN}✅ All recommended dependencies are available.${RESET}"
    echo ""
  fi
}

# ─── Dependency Installation ────────────────────────────────────────────────

install_system_deps() {
  local deps=()
  local pkg_names=()

  section "Checking System Dependencies"

  # Build list of required packages based on detected environment
  _build_dep_list deps

  if [ ${#deps[@]} -eq 0 ]; then
    ok "All system dependencies are satisfied."
    return 0
  fi

  echo ""
  info "Missing packages: ${deps[*]}"

  if [ "$NON_INTERACTIVE" = true ]; then
    info "Non-interactive mode — skipping dependency installation."
    warn "You may need to install: ${deps[*]}"
    return 0
  fi

  echo ""
  echo -n "Install missing packages via ${PKG_MANAGER}? [Y/n] "
  read -r -t 30 install_deps || true

  if [[ "$install_deps" =~ ^[Nn]$ ]]; then
    warn "Skipping dependency installation."
    warn "You may need to install: ${deps[*]}"
    return 0
  fi

  info "Installing dependencies via ${PKG_MANAGER}..."
  _install_deps "${deps[@]}"
}

_build_dep_list() {
  local -n _deps=$1

  # Core runtime dependencies
  case "$PKG_MANAGER" in
    apt)
      # Check each package individually
      dpkg -s libpipewire-0.3-0 &>/dev/null || _deps+=("libpipewire-0.3-0")
      dpkg -s pipewire &>/dev/null || _deps+=("pipewire")
      dpkg -s tailscale &>/dev/null || _deps+=("tailscale")
      ;;
    pacman)
      pacman -Qi pipewire &>/dev/null || _deps+=("pipewire")
      pacman -Qi tailscale &>/dev/null || _deps+=("tailscale")
      ;;
    dnf|yum)
      rpm -q pipewire &>/dev/null || _deps+=("pipewire")
      rpm -q tailscale &>/dev/null || _deps+=("tailscale")
      ;;
    zypper)
      rpm -q pipewire &>/dev/null || _deps+=("pipewire")
      rpm -q tailscale &>/dev/null || _deps+=("tailscale")
      ;;
    apk)
      apk info pipewire &>/dev/null || _deps+=("pipewire")
      apk info tailscale &>/dev/null || _deps+=("tailscale")
      ;;
  esac

  # The server encodes video through the system `ffmpeg` program; the
  # release tarball links no FFmpeg library, so it works with any version.
  # Fedora's own build is ffmpeg-free (RPM Fusion's ffmpeg adds libx264).
  if ! command -v ffmpeg &>/dev/null; then
    case "$PKG_MANAGER" in
      apt|pacman|apk) _deps+=("ffmpeg") ;;
      dnf)
        _deps+=("ffmpeg-free")
        warn "Fedora's ffmpeg-free has no libx264: Pocketdesk encodes in hardware (VAAPI) only. RPM Fusion's ffmpeg adds the software encoder."
        ;;
      *) warn "Install FFmpeg (the ffmpeg program): the server encodes video with it." ;;
    esac
  fi

  # Compositor-specific portal backends
  if [ "$SESSION_TYPE" = "wayland" ]; then
    case "$PKG_MANAGER" in
      apt)
        case "$COMPOSITOR" in
          hyprland) dpkg -s xdg-desktop-portal-hyprland &>/dev/null || _deps+=("xdg-desktop-portal-hyprland") ;;
          gnome)    dpkg -s xdg-desktop-portal-gnome &>/dev/null || _deps+=("xdg-desktop-portal-gnome") ;;
          kde)      dpkg -s xdg-desktop-portal-kde &>/dev/null || _deps+=("xdg-desktop-portal-kde") ;;
          sway|wlroots) dpkg -s xdg-desktop-portal-wlr &>/dev/null || _deps+=("xdg-desktop-portal-wlr") ;;
        esac
        ;;
      pacman)
        case "$COMPOSITOR" in
          hyprland) pacman -Qi xdg-desktop-portal-hyprland &>/dev/null || _deps+=("xdg-desktop-portal-hyprland") ;;
          gnome)    pacman -Qi xdg-desktop-portal-gnome &>/dev/null || _deps+=("xdg-desktop-portal-gnome") ;;
          kde)      pacman -Qi xdg-desktop-portal-kde &>/dev/null || _deps+=("xdg-desktop-portal-kde") ;;
          sway|wlroots) pacman -Qi xdg-desktop-portal-wlr &>/dev/null || _deps+=("xdg-desktop-portal-wlr") ;;
        esac
        ;;
    esac
  fi
}

_install_deps() {
  local deps=("$@")
  local cmd=""

  case "$PKG_MANAGER" in
    apt)
      cmd="sudo apt-get update && sudo apt-get install -y ${deps[*]}"
      ;;
    pacman)
      # Never -Sy alone: refreshing the database without upgrading is a
      # partial upgrade, and a package as large as FFmpeg can then leave
      # libraries mismatched. A stale database is the user's -Syu to run.
      cmd="sudo pacman -S --needed --noconfirm ${deps[*]}"
      ;;
    dnf)
      cmd="sudo dnf install -y ${deps[*]}"
      ;;
    yum)
      cmd="sudo yum install -y ${deps[*]}"
      ;;
    zypper)
      cmd="sudo zypper install -y ${deps[*]}"
      ;;
    apk)
      cmd="sudo apk add ${deps[*]}"
      ;;
    nix)
      cmd="nix-env -iA ${deps[*]}"
      ;;
    *)
      warn "Unknown package manager (${PKG_MANAGER}). Cannot auto-install dependencies."
      warn "Please install manually: ${deps[*]}"
      return 1
      ;;
  esac

  info "Running: ${cmd}"
  if [ "$DRY_RUN" = true ]; then
    warn "[DRY RUN] Would execute: ${cmd}"
    return 0
  fi

  if eval "$cmd"; then
    ok "Dependencies installed successfully."
  else
    warn "Some dependencies may have failed to install."
    warn "Please install manually: ${deps[*]}"
    if [ "$PKG_MANAGER" = "pacman" ]; then
      warn "If pacman could not find a package or download it, its database is stale: run 'sudo pacman -Syu' first, then run this script again."
    fi
  fi
}

# ─── Pre-flight checks ──────────────────────────────────────────────────────

preflight() {
  section "Pre-flight Checks"

  # Detect environment
  detect_os
  detect_compositor
  detect_init_system
  detect_system_capabilities

  # Print full system report
  print_system_report

  # Check required commands
  debug "Checking required commands..."
  for cmd in curl tar sudo; do
    if ! command -v "$cmd" &>/dev/null; then
      fatal "Required command not found: ${cmd}. Install it and try again."
    fi
    debug "  ${cmd}: found"
  done

  # Check architecture
  local arch
  arch=$(uname -m)
  debug "  Architecture: ${arch}"
  if [ "$arch" != "x86_64" ]; then
    warn "Architecture detected: ${arch}"
    warn "Only x86_64 pre-built binaries are available."
    warn "You may need to build from source on this architecture."
    if [ "$NON_INTERACTIVE" = true ]; then
      fatal "Non-interactive mode: cannot prompt for architecture confirmation."
    fi
    read -rp "Continue anyway? [y/N] " confirm
    if [[ ! "$confirm" =~ ^[Yy]$ ]]; then
      echo "Aborted."
      exit 0
    fi
  fi

  # The release binaries are built on Ubuntu 24.04 and need its glibc, 2.39,
  # or newer (packaging/README.md); an older one fails at start with a bare
  # "GLIBC_2.39 not found".
  local glibc
  glibc=$(ldd --version 2>/dev/null | head -n 1 | grep -oE '[0-9]+\.[0-9]+$' || true)
  debug "  glibc: ${glibc:-unknown}"
  if [ -z "$glibc" ]; then
    warn "Could not read the glibc version (a musl system such as Alpine?)."
    warn "The release binaries need glibc 2.39 or newer; elsewhere, build from source (docs/guide.md, Build from Source)."
  elif [ "$(printf '%s\n' 2.39 "$glibc" | sort -V | head -n 1)" != "2.39" ]; then
    fatal "This system has glibc ${glibc}; the release binaries need 2.39 or newer (Ubuntu 24.04, Debian 13, Fedora 40, Arch and later). Build from source instead (docs/guide.md, Build from Source)."
  fi

  # Check sudo access
  debug "Checking sudo access..."
  if ! sudo -n true 2>/dev/null; then
    info "Verifying sudo access (will prompt for password if needed)..."
    if ! sudo -v; then
      fatal "Cannot obtain sudo access. This installer needs root privileges."
    fi
  fi
  debug "  sudo: OK"

  # Check disk space (need ~100 MB for download + install)
  if command -v df &>/dev/null; then
    local available_kb
    available_kb=$(df -k "$INSTALL_PREFIX" 2>/dev/null | awk 'NR==2 {print $4}')
    debug "  Free space on ${INSTALL_PREFIX}: ${available_kb} KB"
    if [ -n "$available_kb" ] && [ "$available_kb" -lt 102400 ]; then
      fatal "Less than 100 MB free on ${INSTALL_PREFIX} (${available_kb} KB). Free up space and try again."
    fi
  fi

  # Check for existing installation
  if [ -f "${INSTALL_PREFIX}/bin/${BINARY_NAME}" ]; then
    CURRENT_VERSION=$("${INSTALL_PREFIX}/bin/${BINARY_NAME}" --version 2>/dev/null || echo "unknown")
    warn "Existing installation: ${BINARY_NAME} ${CURRENT_VERSION}"
    UPGRADE=true
  fi

  # Install missing dependencies
  install_system_deps

  ok "Pre-flight checks passed."
}

# ─── Determine version ───────────────────────────────────────────────────────

resolve_version() {
  if [ "$VERSION" = "latest" ]; then
    info "Fetching latest release from GitHub..."
    VERSION=$(fetch_latest_version) || {
      warn "GitHub API not reachable, trying HTML fallback..."
      local html
      html=$(curl -fsSL --max-time 15 "https://github.com/${REPO_OWNER}/${REPO_NAME}/releases" 2>/dev/null) || \
        fatal "Cannot reach GitHub. Check your internet connection and DNS."
      VERSION=$(echo "$html" | grep -o '/releases/tag/[^"]*' | head -1 | sed 's|/releases/tag/||') || \
        fatal "Cannot determine latest version from GitHub."
    }

    if [ -z "$VERSION" ]; then
      fatal "Could not determine the latest version. The repository may have no releases yet."
    fi

    ok "Latest version: ${VERSION}"
  else
    # Normalize version: ensure it starts with 'v'
    if [[ ! "$VERSION" =~ ^v[0-9] ]]; then
      VERSION="v${VERSION}"
    fi
    info "Using version: ${VERSION}"
  fi
}

# ─── Install command ────────────────────────────────────────────────────────

cmd_install() {
  resolve_paths
  local prev_version=""

  # Check if already at target version
  if [ "$UPGRADE" = true ] && [ "$FORCE" != true ] && [ "${CURRENT_VERSION:-unknown}" = "$VERSION" ]; then
    ok "${BINARY_NAME} ${VERSION} is already installed. Use --force to reinstall."
    return 0
  fi

  # Save previous version for rollback support
  if [ "$UPGRADE" = true ]; then
    prev_version="${CURRENT_VERSION}"
  elif load_state 2>/dev/null; then
    prev_version="${version:-}"
  fi

  section "Downloading"

  # Build download URL
  # The release workflow's names: pocketdesk-<tag>-linux-x86_64.tar.gz, with
  # every file's checksum in one SHA256SUMS.
  local release_dir="pocketdesk-${VERSION}-linux-x86_64"
  local archive_name="${release_dir}.tar.gz"
  local url="https://github.com/${REPO_OWNER}/${REPO_NAME}/releases/download/${VERSION}/${archive_name}"

  TMPDIR=$(mktemp -d)
  debug "Temp directory: ${TMPDIR}"

  info "Downloading ${BINARY_NAME} ${VERSION}..."
  debug "URL: ${url}"

  # Respect proxy settings
  local curl_opts=(-fsSL --max-time 120 --retry 3 --retry-delay 5 -w '%{http_code}')

  # Add proxy if set
  if [ -n "${HTTPS_PROXY:-}" ]; then
    curl_opts+=(--proxy "$HTTPS_PROXY")
  elif [ -n "${HTTP_PROXY:-}" ]; then
    curl_opts+=(--proxy "$HTTP_PROXY")
  fi

  local http_code
  http_code=$(curl "${curl_opts[@]}" -o "${TMPDIR}/${archive_name}" "$url" 2>/dev/null) || {
    if curl -fsS --max-time 5 -o /dev/null "https://github.com" 2>/dev/null; then
      fatal "Failed to download release. Check available releases at https://github.com/${REPO_OWNER}/${REPO_NAME}/releases"
    else
      fatal "Network unavailable. Check your internet connection."
    fi
  }

  if [ "$http_code" = "404" ]; then
    fatal "Release ${VERSION} not found (HTTP 404). Check https://github.com/${REPO_OWNER}/${REPO_NAME}/releases"
  elif [ "$http_code" != "200" ]; then
    fatal "Download failed with HTTP ${http_code}."
  fi

  # Verify archive size
  local archive_size
  archive_size=$(stat -c%s "${TMPDIR}/${archive_name}" 2>/dev/null || stat -f%z "${TMPDIR}/${archive_name}" 2>/dev/null || echo "0")
  if [ "$archive_size" -lt 100 ]; then
    fatal "Downloaded file is too small (${archive_size} bytes). Release may be corrupted."
  fi
  debug "Archive size: ${archive_size} bytes"

  # Checksum verification
  local checksum_url="https://github.com/${REPO_OWNER}/${REPO_NAME}/releases/download/${VERSION}/SHA256SUMS"
  local checksum_file="${TMPDIR}/checksums.txt"
  if curl -fsSL --max-time 15 -o "$checksum_file" "$checksum_url" 2>/dev/null; then
    local expected
    expected=$(grep "$archive_name" "$checksum_file" 2>/dev/null | awk '{print $1}' || true)
    if [ -n "$expected" ]; then
      local actual=""
      if command -v sha256sum &>/dev/null; then
        actual=$(sha256sum "${TMPDIR}/${archive_name}" | awk '{print $1}')
      elif command -v shasum &>/dev/null; then
        actual=$(shasum -a 256 "${TMPDIR}/${archive_name}" | awk '{print $1}')
      else
        warn "No sha256sum or shasum found — skipping checksum verification."
      fi
      if [ -n "$actual" ] && [ "$actual" != "$expected" ]; then
        fatal "Checksum mismatch! Expected: ${expected}, Got: ${actual}. File may be corrupted or tampered."
      else
        ok "Checksum verified."
      fi
    fi
  else
    debug "No checksums file found — skipping verification."
  fi

  # Extract
  section "Extracting"
  info "Extracting archive..."
  if ! tar xzf "${TMPDIR}/${archive_name}" -C "$TMPDIR" 2>/dev/null; then
    fatal "Failed to extract archive. File may be corrupted."
  fi

  # Find extracted directory
  local extracted_dir="${TMPDIR}/${release_dir}"
  if [ ! -d "$extracted_dir" ]; then
    extracted_dir=$(find "$TMPDIR" -maxdepth 1 -type d -not -name "$(basename "$TMPDIR")" | head -1)
    if [ -z "$extracted_dir" ]; then
      fatal "Archive extracted but expected directory structure not found."
    fi
  fi
  debug "Extracted to: ${extracted_dir}"

  # Verify binary
  if [ ! -f "${extracted_dir}/${BINARY_NAME}" ]; then
    fatal "Binary '${BINARY_NAME}' not found in release archive."
  fi
  if ! file "${extracted_dir}/${BINARY_NAME}" | grep -q "ELF"; then
    fatal "Binary is not a valid ELF executable. Release may be for wrong architecture."
  fi
  ok "Archive verified."

  # Install
  section "Installing"

  # Stop service before upgrade
  if [ "$UPGRADE" = true ] && command -v systemctl &>/dev/null && user_session_live \
    && user_systemctl is-active --quiet "$BINARY_NAME" 2>/dev/null; then
    info "Stopping service for upgrade..."
    [ "$DRY_RUN" = true ] || user_systemctl stop "$BINARY_NAME"
  fi

  # Backup config before upgrade
  if [ "$UPGRADE" = true ] && [ -f "$CONFIG_FILE" ]; then
    [ "$DRY_RUN" = true ] || mkdir -p "$BACKUP_DIR"
    local backup_file="${BACKUP_DIR}/config.toml.bak.$(date +%Y%m%d%H%M%S)"
    info "Backing up config to ${backup_file}..."
    [ "$DRY_RUN" = true ] || cp "$CONFIG_FILE" "$backup_file"

    # Keep only last 5 backups
    if [ -d "$BACKUP_DIR" ] && [ "$DRY_RUN" != true ]; then
      local backup_count
      backup_count=$(ls -1 "$BACKUP_DIR"/config.toml.bak.* 2>/dev/null | wc -l)
      if [ "$backup_count" -gt 5 ]; then
        ls -1t "$BACKUP_DIR"/config.toml.bak.* | tail -n +6 | xargs rm -f
        debug "Cleaned up old backups (kept 5 most recent)."
      fi
    fi
  fi

  # Install binary
  info "Installing binary to ${INSTALL_PREFIX}/bin/${BINARY_NAME}..."
  [ "$DRY_RUN" = true ] || sudo install -Dm755 "${extracted_dir}/${BINARY_NAME}" "${INSTALL_PREFIX}/bin/${BINARY_NAME}"
  # The desktop app and its tray, when the release carries them.
  local app
  for app in pocketdesk-app pocketdesk-tray; do
    if [ -f "${extracted_dir}/${app}" ]; then
      info "Installing ${app} to ${INSTALL_PREFIX}/bin/${app}..."
      [ "$DRY_RUN" = true ] || sudo install -Dm755 "${extracted_dir}/${app}" "${INSTALL_PREFIX}/bin/${app}"
    fi
  done

  # Install service (systemd *user* unit — the server needs the graphical
  # session's Wayland/PipeWire, which a system unit cannot reach)
  if [ "$NO_SERVICE" = true ]; then
    info "Skipping service installation (--no-service)."
  elif [ -f "${extracted_dir}/${BINARY_NAME}.service" ]; then
    info "Installing systemd user service for ${SERVICE_USER}..."
    if [ "$DRY_RUN" != true ]; then
      sudo install -d -o "$SERVICE_USER" -g "$SERVICE_GROUP" -m755 \
        "${SERVICE_USER_HOME}/.config/systemd/user"
      sed "s|^ExecStart=.*|ExecStart=${INSTALL_PREFIX}/bin/${BINARY_NAME} start|" \
        "${extracted_dir}/${BINARY_NAME}.service" | sudo tee "$SERVICE_FILE" >/dev/null
      sudo chmod 644 "$SERVICE_FILE"
      user_systemctl daemon-reload || true
    fi
  else
    warn "Service file not found in release archive."
  fi

  # Install docs
  if [ "$NO_DOCS" = true ]; then
    info "Skipping documentation installation (--no-docs)."
  else
    local doc_dir="${INSTALL_PREFIX}/share/doc/${BINARY_NAME}"
    for doc in README.md SECURITY.md LICENSE-MIT LICENSE-APACHE config.toml.example; do
      if [ -f "${extracted_dir}/${doc}" ]; then
        [ "$DRY_RUN" = true ] || sudo install -Dm644 "${extracted_dir}/${doc}" "${doc_dir}/${doc}"
      fi
    done
  fi

  # Install man page
  if [ "$NO_MAN" = true ]; then
    info "Skipping man page installation (--no-man)."
  elif [ -f "${extracted_dir}/man/${BINARY_NAME}.1" ]; then
    info "Installing man page..."
    [ "$DRY_RUN" = true ] || sudo install -Dm644 "${extracted_dir}/man/${BINARY_NAME}.1" \
      "${INSTALL_PREFIX}/share/man/man1/${BINARY_NAME}.1"
    [ "$DRY_RUN" = true ] || mandb -q 2>/dev/null || true
  fi

  # Setup config directory
  if [ "$NO_CONFIG" = true ]; then
    info "Skipping config setup (--no-config)."
  else
    if [ ! -d "$CONFIG_DIR" ]; then
      info "Creating config directory ${CONFIG_DIR}..."
      [ "$DRY_RUN" = true ] || mkdir -p "$CONFIG_DIR"
    fi

    if [ ! -f "$CONFIG_FILE" ] && [ -f "${extracted_dir}/config.toml.example" ]; then
      info "Installing default config to ${CONFIG_FILE}..."
      [ "$DRY_RUN" = true ] || cp "${extracted_dir}/config.toml.example" "$CONFIG_FILE"
    fi
  fi

  # Save install state
  if [ "$DRY_RUN" != true ]; then
    save_state "$VERSION" "$prev_version"
  fi

  # Post-install: enable and start the user service
  section "Post-install"
  if [ "$NO_SERVICE" = true ]; then
    info "Skipping service setup (--no-service)."
  elif command -v systemctl &>/dev/null; then
    if ! user_session_live; then
      warn "No live systemd session for user '${SERVICE_USER}' — cannot enable the"
      warn "user service from here. Unit file is installed; from the desktop session run:"
      warn "  systemctl --user enable --now ${BINARY_NAME}"
    elif [ "$UPGRADE" = true ]; then
      info "Restarting ${BINARY_NAME} service..."
      [ "$DRY_RUN" = true ] || user_systemctl restart "$BINARY_NAME"
    elif [ "$NON_INTERACTIVE" = true ]; then
      info "Non-interactive mode — enabling service..."
      [ "$DRY_RUN" = true ] || user_systemctl enable "$BINARY_NAME"
      [ "$DRY_RUN" = true ] || user_systemctl start "$BINARY_NAME"
    else
      echo ""
      echo -n "Enable and start ${BINARY_NAME} service (user session)? [Y/n] "
      read -r -t 10 enable_service || true
      if [[ ! "$enable_service" =~ ^[Nn]$ ]]; then
        info "Enabling and starting ${BINARY_NAME} service..."
        [ "$DRY_RUN" = true ] || user_systemctl enable "$BINARY_NAME"
        [ "$DRY_RUN" = true ] || user_systemctl start "$BINARY_NAME"
      else
        info "Service not started. Run 'systemctl --user enable --now ${BINARY_NAME}' manually."
      fi
    fi

    # Verify service health
    if [ "$DRY_RUN" != true ] && user_session_live \
      && user_systemctl is-active --quiet "$BINARY_NAME" 2>/dev/null; then
      ok "Service is running."
    fi
  elif [ -d /etc/init.d ] && command -v rc-status &>/dev/null; then
    # OpenRC
    info "Detected OpenRC — setting up init script..."
    if [ -f "${extracted_dir}/${BINARY_NAME}.openrc" ]; then
      [ "$DRY_RUN" = true ] || sudo cp "${extracted_dir}/${BINARY_NAME}.openrc" /etc/init.d/${BINARY_NAME}
      [ "$DRY_RUN" = true ] || sudo chmod +x /etc/init.d/${BINARY_NAME}
      [ "$DRY_RUN" = true ] || sudo rc-update add ${BINARY_NAME} default
      [ "$DRY_RUN" = true ] || sudo rc-service ${BINARY_NAME} start
      ok "OpenRC service configured."
    else
      warn "No OpenRC init script found in release archive."
      warn "Create /etc/init.d/${BINARY_NAME} manually or use systemd."
    fi
  elif command -v sv &>/dev/null; then
    # runit
    info "Detected runit — service setup requires manual configuration."
    warn "Create /etc/sv/${BINARY_NAME}/run manually."
  else
    warn "No supported init system detected — service will not be managed automatically."
    warn "Start manually with: ${BINARY_NAME} start"
  fi

  # Post-install health check
  section "Health Check"
  local installed_ver
  installed_ver=$("${INSTALL_PREFIX}/bin/${BINARY_NAME}" --version 2>/dev/null || echo "failed")
  if [ "$installed_ver" != "failed" ]; then
    ok "Binary responds: ${installed_ver}"
  else
    warn "Binary did not respond to --version."
  fi

  # Summary
  section "Summary"
  echo ""
  echo -e "${GREEN}${BOLD}Installation complete!${RESET}"
  echo ""
  echo "  Binary:   ${INSTALL_PREFIX}/bin/${BINARY_NAME} (${installed_ver})"
  echo "  Config:   ${CONFIG_FILE}"
  echo "  Service:  ${SERVICE_FILE} (systemd --user)"
  echo ""
  echo "Next steps:"
  echo "  1. Edit config:   nano ${CONFIG_FILE}"
  echo "  2. Start daemon:  systemctl --user enable --now ${BINARY_NAME}"
  echo "  3. Check status:  systemctl --user status ${BINARY_NAME}"
  echo "  4. View logs:     journalctl --user -u ${BINARY_NAME} -f"
  echo "  5. Read docs:     man ${BINARY_NAME}"
  echo ""

  if [ "$DRY_RUN" = true ]; then
    echo -e "${YELLOW}${BOLD}DRY RUN — no changes were made.${RESET}"
    echo ""
  fi
}

# ─── Main ────────────────────────────────────────────────────────────────────

main() {
  parse_args "$@"
  resolve_paths

  # Handle standalone commands (don't need install flow)
  if [ "$LIST_VERSIONS" = true ]; then
    cmd_list_versions
    exit 0
  fi

  if [ "$SHOW_STATUS" = true ]; then
    cmd_status
    exit 0
  fi

  if [ "$CHECK_UPDATES" = true ]; then
    cmd_check_updates
    exit 0
  fi

  if [ "$UNINSTALL" = true ]; then
    cmd_uninstall
    exit 0
  fi

  if [ "$ROLLBACK" = true ]; then
    cmd_rollback
    exit 0
  fi

  # Full install flow
  preflight
  resolve_version
  cmd_install
}

main "$@"
