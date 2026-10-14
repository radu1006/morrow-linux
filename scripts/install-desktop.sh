#!/bin/sh
# Turn an existing Alpine installation into the Morrow desktop.
#
# This is the "I already run Alpine" path, and it installs exactly what the ISO bakes in:
# scripts/morrow-packages.sh is the one list both of them read. Run it as root, naming the
# account that will start the session:
#
#     MORROW_USER=you sh scripts/install-desktop.sh
#
# Set MORROW_LEAN=1 on a machine with 500 MB or less. The panel and the desktop icons are the
# two largest things in this session after X itself, and the window manager's own menu still
# reaches a terminal, the files and the monitor without them.
set -eu

here="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=scripts/morrow-packages.sh
. "$here/morrow-packages.sh"

if [ "$(id -u)" -ne 0 ]; then
    echo "Run this script with doas or sudo." >&2
    exit 1
fi

if [ ! -f /etc/alpine-release ]; then
    echo "Morrow's desktop setup currently targets Alpine Linux." >&2
    exit 1
fi

APP="${MORROW_APP:-$here/../target/release/morrow-watch}"
if [ ! -x "$APP" ]; then
    cat >&2 <<EOF
Missing release binary: $APP

Build it on Alpine, where it will run:

    apk add $MORROW_BUILDER_APKS
    rustup-init -y --default-toolchain stable --profile minimal
    export PATH="$HOME/.cargo/bin:$PATH"
    cargo build --release

A binary built on a glibc distribution will not start here, and a Windows build is a
development artefact: it proves the monitor reads a machine correctly, not that it can be
shipped. Point MORROW_APP at a musl build if it lives somewhere else.
EOF
    exit 1
fi

TARGET_USER="${MORROW_USER:-${SUDO_USER:-}}"
if [ -z "$TARGET_USER" ] || ! grep -q "^$TARGET_USER:" /etc/passwd; then
    echo "Set MORROW_USER to an existing non-root account before running this script." >&2
    exit 1
fi
if [ "$TARGET_USER" = root ]; then
    echo "A graphical session should not run as root; set MORROW_USER to a normal account." >&2
    exit 1
fi

# shellcheck disable=SC2086  # a list on purpose: it is word-split into separate arguments.
apk add --no-cache $MORROW_APKS
morrow_install_fonts
morrow_enable_services

# The monitor goes to /usr/local/bin: on every Alpine machine, outside the package manager, and
# overwritten on every run, because replacing it is the reason to run this at all.
mkdir -p /usr/local/bin
cp "$APP" /usr/local/bin/morrow-watch
chmod 0755 /usr/local/bin/morrow-watch

# Everything that is not the binary comes from overlay/, which the ISO builder reads too. Both
# paths therefore install the same session, the same X input class and the same desktop entry, and
# a person who has done both can diff the two machines and find nothing to explain.
MORROW_OVERLAY="${MORROW_OVERLAY:-$here/../overlay}"
if [ ! -d "$MORROW_OVERLAY" ]; then
    echo "Missing the overlay directory: $MORROW_OVERLAY" >&2
    exit 1
fi

# Copy a file into place only if it is not there already. An installer has no business
# overwriting a config file it did not write: re-running this script should be safe, and a
# session someone has already tuned should survive it.
copy_missing() {
    source="$1"
    target="$2"
    mode="$3"
    if [ -e "$target" ]; then
        echo "  keeping the existing $target"
        return 0
    fi
    mkdir -p "$(dirname "$target")"
    cp "$source" "$target"
    chmod "$mode" "$target"
}

USER_HOME="$(awk -F: -v user="$TARGET_USER" '$1 == user { print $6 }' /etc/passwd)"
USER_GROUP="$(id -gn "$TARGET_USER")"
if [ -z "$USER_HOME" ] || [ ! -d "$USER_HOME" ]; then
    echo "Could not find a home directory for $TARGET_USER." >&2
    exit 1
fi

for relative in .xinitrc .config/openbox/autostart .config/openbox/menu.xml; do
    copy_missing "$MORROW_OVERLAY/skel/$relative" "$USER_HOME/$relative" 0755
done
copy_missing "$MORROW_OVERLAY/etc/X11/xorg.conf.d/20-morrow.conf" \
    /etc/X11/xorg.conf.d/20-morrow.conf 0644
copy_missing "$MORROW_OVERLAY/usr/share/applications/morrow-watch.desktop" \
    /usr/share/applications/morrow-watch.desktop 0644

# The lean marker is what .xinitrc reads, so the choice is recorded once, on disk, instead of being
# repeated in every script that wants to know about it.
if [ "${MORROW_LEAN:-0}" = 1 ]; then
    mkdir -p "$USER_HOME/.config/morrow"
    : > "$USER_HOME/.config/morrow/lean"
fi

# Reading the video and audio devices is not a privilege worth handing to every program, so the
# session gets the groups that grant exactly those and nothing more.
for group in video input audio; do
    if grep -q "^$group:" /etc/group; then
        addgroup "$TARGET_USER" "$group" 2>/dev/null || true
    fi
done

# NetworkManager is now the network manager, but Alpine's base install also has a `networking`
# service reading /etc/network/interfaces, and two managers on one interface produce a machine that
# looks connected and is not. The overlay supplies a loopback-only file for a machine Morrow sets
# up; an installation that already configures an interface is left alone, with a sentence about the
# one command that would settle the argument.
if awk '$2 != "lo" && ($1 == "auto" || $1 == "iface") { found = 1 } END { exit !found }' \
    /etc/network/interfaces 2>/dev/null; then
    cat >&2 <<'EOF'
Note: /etc/network/interfaces configures an interface other than loopback, and NetworkManager
      manages those interfaces too. If the connection misbehaves, move that configuration into
      nmtui and run `rc-update del networking default`.
EOF
fi

chown -R "$TARGET_USER:$USER_GROUP" "$USER_HOME/.config" "$USER_HOME/.xinitrc"
rc-service NetworkManager start 2>/dev/null || true

cat <<EOF
Morrow desktop installed.

Log in as $TARGET_USER and start the session with:

    startx

The session is Openbox, Tint2, PCManFM and Morrow Watch; on a machine with 500 MB or less,
re-run this script with MORROW_LEAN=1 to leave out the panel and the desktop icons.
EOF
