#!/bin/sh
# The /etc that goes into Morrow's apkovl.
#
# Alpine's image builder runs this file as:
#
#     (cd $DESTDIR; fakeroot genapkovl-morrow.sh <hostname>)
#
# so the hostname arrives as $1 and the archive must be written into the current directory, which is
# the directory being assembled. Everything the image boots with that is not a package travels in
# that archive: the monitor, the session files and the package list. Morrow is not in a repository,
# so the overlay *is* the distribution.
#
# Everything under overlay/ is shared with scripts/install-desktop.sh, so an installed desktop and
# a booted ISO cannot end up with different configuration.
set -eu

die() {
    echo "genapkovl-morrow: $*" >&2
    exit 1
}

if [ "${1:-}" = "-h" ] || [ "${1:-}" = "--help" ]; then
    # The comment block at the top of this file is the documentation, so the help is that block.
    sed -n '2,/^set /p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'
    exit 0
fi

repo="${MORROW_REPO:-$(cd "$(dirname "$0")/../.." && pwd)}"
if [ ! -d "$repo/overlay" ]; then
    die "no overlay directory under $repo"
fi
# shellcheck source=scripts/morrow-packages.sh
. "$repo/scripts/morrow-packages.sh"

# $1 first: the builder names the archive after the hostname it passes, and an archive named
# anything else is an archive the builder will not find.
hostname="${1:-${MORROW_HOSTNAME:-${hostname:-morrow}}}"

tmp="$(mktemp -d)"
cleanup() {
    rm -rf "$tmp"
}
trap cleanup EXIT INT TERM

# 1. The session, as /etc/skel so that an account created on the running system has a desktop
#    without anyone having to run the installer.
mkdir -p "$tmp/etc/skel"
cp -a "$repo/overlay/skel/." "$tmp/etc/skel/"

# The session files are read by programs that may or may not consult the shebang line, and a
# non-executable .xinitrc is a `startx` that does nothing at all. One file per chmod, so that the
# mode of one cannot depend on the mode of another.
for session_file in .xinitrc .config/openbox/autostart; do
    chmod 0755 "$tmp/etc/skel/$session_file"
done

# A lean image, if one was asked for. The marker is a file so that the session reads it once.
if [ "${MORROW_LEAN:-0}" = 1 ]; then
    mkdir -p "$tmp/etc/skel/.config/morrow"
    : > "$tmp/etc/skel/.config/morrow/lean"
fi

# 2. Machine-wide files: X's input class and the monitor's desktop entry.
cp -a "$repo/overlay/etc/." "$tmp/etc/"
cp -a "$repo/overlay/usr/." "$tmp/usr/"

# 3. The monitor itself. It has to be a musl binary: the builder will happily copy a glibc build
#    into the image, and the failure then appears on the target machine, hours later, as "not
#    found" on a program that is plainly there.
binary="${MORROW_BINARY:-$repo/target/release/morrow-watch}"
if [ ! -x "$binary" ]; then
    die "no monitor binary at $binary (set MORROW_BINARY)"
fi
mkdir -p "$tmp/usr/local/bin"
cp "$binary" "$tmp/usr/local/bin/morrow-watch"
chmod 0755 "$tmp/usr/local/bin/morrow-watch"

# 4. Services. dbus and NetworkManager are what the desktop expects to find already running, and
#    udev has to be up before X looks for a keyboard.
mkdir -p "$tmp/etc/runlevels/sysinit" "$tmp/etc/runlevels/default"

# Symlinks, not copies: a runlevel entry has to be the same file as the init script it starts. This
# is the one step in here that a Windows checkout cannot exercise -- `ln -s` wants a real symlink --
# so it says plainly which link failed rather than dying with a generic message.
link_service() {
    runlevel="$1"
    service="$2"
    ln -sf "/etc/init.d/$service" "$tmp/etc/runlevels/$runlevel/$service" ||
        die "could not link $service into the $runlevel runlevel"
}

for service in $MORROW_SYSINIT_SERVICES; do
    link_service sysinit "$service"
done
for service in $MORROW_DEFAULT_SERVICES; do
    link_service default "$service"
done

# ...but not the base image's own networking, which would come up first and fight NetworkManager
# for the same interface. The loopback-only file comes from the overlay, like the rest of /etc.
# (The installer leaves an existing /etc/network/interfaces alone and says so instead; an image
# has no previous owner to ask.)
if [ ! -f "$tmp/etc/network/interfaces" ]; then
    die "overlay/etc/network/interfaces is missing"
fi

# 5. The package list, so that an upgrade on the installed system keeps the desktop together
#    instead of quietly removing the parts of it nobody has used yet.
mkdir -p "$tmp/etc/apk"
{
    printf 'alpine-base\n'
    for apk in $MORROW_APKS; do
        printf '%s\n' "$apk"
    done
    if [ -n "${MORROW_FONT:-}" ]; then
        printf '%s\n' "$MORROW_FONT"
    fi
} > "$tmp/etc/apk/world"

# The archive itself, into the directory the builder ran us in. It looks for it under the hostname it
# passed, and treats a missing archive as a failed build.
tar -C "$tmp" -cf - . | gzip -9 > "$hostname.apkovl.tar.gz"
echo "genapkovl-morrow: wrote $PWD/$hostname.apkovl.tar.gz ($(wc -c < "$hostname.apkovl.tar.gz") bytes)"
