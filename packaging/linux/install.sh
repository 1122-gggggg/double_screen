#!/usr/bin/env bash
# Install splitdeskd + splitdesk and the systemd unit.
# The unit starts the daemon, never a compositor. Weston is spawned as the
# session UID at create-time (systemd-run --uid=), not as root.
set -euo pipefail

if [[ "$(id -u)" -ne 0 ]]; then
  echo "install.sh: run as root to install binaries and the unit" >&2
  exit 1
fi

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
PREFIX="${PREFIX:-/usr/local}"
UNIT_DIR="${UNIT_DIR:-/etc/systemd/system}"
SPLITDESKD="${SPLITDESKD:-$ROOT/target/release/splitdeskd}"
SPLITDESK="${SPLITDESK:-$ROOT/target/release/splitdesk}"
UNIT_SRC="$ROOT/packaging/linux/splitdesk.service"
MODULE_SRC="$ROOT/native/weston-input"
MODULE_BUILD="${MODULE_BUILD:-$ROOT/target/weston-input}"
MODULE_DST="$PREFIX/lib/splitdesk/splitdesk-input.so"

die() { echo "install.sh: $*" >&2; exit 1; }

[[ -f "$UNIT_SRC" ]] || die "missing $UNIT_SRC"
if grep -Eq '^[[:space:]]*ExecStart=.*(weston|mutter|sway|kwin)' "$UNIT_SRC"; then
  die "refusing unit that ExecStart's a compositor (must start splitdeskd only)"
fi
if grep -Eq '^[[:space:]]*ExecStart=.*0\.0\.0\.0' "$UNIT_SRC"; then
  die "refusing unit that binds 0.0.0.0 by default"
fi

[[ -x "$SPLITDESKD" ]] || die "missing $SPLITDESKD (cargo build -p splitdeskd --release)"
[[ -x "$SPLITDESK" ]] || die "missing $SPLITDESK (cargo build -p splitdesk-cli --release)"
command -v meson >/dev/null 2>&1 || die "meson is required to build the Weston input module"
command -v ninja >/dev/null 2>&1 || die "ninja is required to build the Weston input module"

if [[ -f "$MODULE_BUILD/build.ninja" ]]; then
  meson setup --reconfigure "$MODULE_BUILD" "$MODULE_SRC" --prefix "$PREFIX" --libdir lib
else
  meson setup "$MODULE_BUILD" "$MODULE_SRC" --prefix "$PREFIX" --libdir lib --buildtype release
fi
meson compile -C "$MODULE_BUILD"
[[ -f "$MODULE_BUILD/splitdesk-input.so" ]] || die "Weston input module build produced no .so"

install -D -m 0755 "$SPLITDESKD" "$PREFIX/bin/splitdeskd"
install -D -m 0755 "$SPLITDESK" "$PREFIX/bin/splitdesk"
install -D -m 0755 "$MODULE_BUILD/splitdesk-input.so" "$MODULE_DST"
install -D -m 0644 "$UNIT_SRC" "$UNIT_DIR/splitdesk.service"

# Runtime / token directories: 0700, never 0777.
install -d -m 0700 /tmp/splitdesk-"$(id -u)"
if [[ -n "${XDG_RUNTIME_DIR:-}" ]]; then
  install -d -m 0700 "$XDG_RUNTIME_DIR/splitdesk"
fi

if command -v systemctl >/dev/null 2>&1; then
  systemctl daemon-reload
fi

echo "installed: $PREFIX/bin/splitdeskd $PREFIX/bin/splitdesk $MODULE_DST $UNIT_DIR/splitdesk.service"
echo "token files must stay mode 0600; dirs 0700."
echo "enable: systemctl enable --now splitdesk"
echo "Weston is not started by this unit; sessions spawn it as the session user."
