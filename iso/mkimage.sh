#!/bin/sh
# Build a bootable Morrow ISO.
#
# This script does not build an ISO. Alpine's own `mkimage.sh` does, out of the aports tree, and
# this is the driver around it: it installs what the builder needs, checks out aports at the
# release you name, drops Morrow's profile next to Alpine's, builds the monitor as a musl binary
# and hands over. Everything Morrow-specific is in iso/profile/.
#
#     MORROW_BRANCH=3.20 sh iso/mkimage.sh
#     MORROW_BRANCH=edge MORROW_LEAN=1 sh iso/mkimage.sh --skip-build
#
# Run it on Alpine, or inside an Alpine container: the builder calls `apk --root` and needs root,
# and the monitor has to be linked against musl or it will not start on the machine that boots
# the ISO.
set -eu

here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/.." && pwd)"
# shellcheck source=scripts/morrow-packages.sh
. "$repo/scripts/morrow-packages.sh"

die() {
    echo "mkimage: $*" >&2
    exit 1
}

usage_error() {
    echo "mkimage: $*" >&2
    echo "Try 'sh iso/mkimage.sh --help'." >&2
    exit 2
}

skip_build=0
for arg in "$@"; do
    case "$arg" in
        --skip-build) skip_build=1 ;;
        -h | --help)
            # The comment block at the top of this file is the documentation, so the help is that
            # block itself: it cannot go stale while someone keeps the file readable.
            sed -n '2,/^set /p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        *) usage_error "unknown argument '$arg'" ;;
    esac
done

[ -f /etc/alpine-release ] || die "run this on Alpine: the image builder needs apk"

branch="${MORROW_BRANCH:-$(cut -d. -f1,2 /etc/alpine-release)}"
mirror="${MORROW_MIRROR:-https://dl-cdn.alpinelinux.org/alpine}"
arch="${MORROW_ARCH:-$(apk --print-arch)}"
lean="${MORROW_LEAN:-0}"

# Alpine names its release directories `v3.20` but `edge`, so a tag that does not match the mirror
# fails at the first package download, halfway into a build.
case "$branch" in
    edge) tag="edge" ;;
    *) tag="v$branch" ;;
esac
case "$branch" in
    edge) aports_branch="edge" ;;
    *) aports_branch="${branch}-stable" ;;
esac

aports="$here/aports"
outdir="${MORROW_OUT:-$here/dist}"
work="${MORROW_WORK:-$here/.cache}"

echo "Morrow $branch ($tag) on $arch (lean=$lean)"
echo "  profile: $here/profile/mkimg.morrow.sh"
echo "  aports:  $aports"
echo "  output:  $outdir"

# 1. What the builder needs. Upstream's own list is in the comment at the top of scripts/mkimage.sh
#    -- abuild for the index it signs, alpine-conf for update-kernel, the image tools for each boot
#    format -- and alpine-sdk is what its instructions name. The Rust toolchain and a C compiler are
#    Morrow's: the monitor has to be linked against musl here, on this machine.
# shellcheck disable=SC2086  # a list on purpose: it is word-split into separate arguments.
apk add --no-cache $MORROW_BUILDER_APKS alpine-sdk \
    abuild alpine-conf fakeroot grub-efi mtools xorriso squashfs-tools syslinux cpio >/dev/null
morrow_setup_rust

# 2. The aports tree, at the release the image will name. A shallow clone: mkimage.sh only reads
#    the scripts, and a full history is 200 MB of branch names nobody will look at.
if [ ! -d "$aports/scripts" ]; then
    echo "Cloning aports $aports_branch..."
    git clone --depth 1 --branch "$aports_branch" \
        https://gitlab.alpinelinux.org/alpine/aports.git "$aports"
else
    echo "Using the existing aports checkout in $aports"
fi

# 3. The profile, copied rather than linked: a later edit to the checkout should not change what
#    this build did. mkimage.sh looks for profiles beside itself.
install -m 0644 "$here/profile/mkimg.morrow.sh" "$aports/scripts/mkimg.morrow.sh"
install -m 0755 "$here/profile/genapkovl-morrow.sh" "$aports/scripts/genapkovl-morrow.sh"

# Morrow's profile inherits Alpine's own, so it is worth failing here, with a sentence explaining
# what to change, rather than in the middle of a build that has already written a filesystem. The
# patterns are the ones mkimage.sh itself uses to find a profile: `^profile_<name>() {`, at the start
# of a line, in a mkimg.*.sh beside the builder.
grep -q '^profile_standard() {' "$aports/scripts/mkimg.standard.sh" 2>/dev/null &&
    grep -q '^profile_base() {' "$aports/scripts/mkimg.base.sh" 2>/dev/null ||
    die "aports no longer defines profile_standard()/profile_base(); update iso/profile/mkimg.morrow.sh"

# 4. The monitor. A glibc build would be copied into the image without complaint and then fail on
#    the target, so it is built here, on musl, unless a binary was handed over.
binary="${MORROW_BINARY:-$repo/target/release/morrow-watch}"
if [ "$skip_build" = 1 ]; then
    echo "Skipping the build; expecting $binary"
elif [ -x "$binary" ] && [ "${MORROW_REBUILD:-0}" != 1 ]; then
    echo "Using the existing $binary (MORROW_REBUILD=1 rebuilds it)"
else
    echo "Building the monitor with musl..."
    (cd "$repo" && cargo build --release)
fi
[ -x "$binary" ] || die "no monitor binary at $binary"

# 5. A signing key. The ISO carries a package repository of its own -- the packages that came with
#    the disc -- and mkimage.sh signs that repository's index with abuild-sign, which refuses to run
#    without a key ("No private key found"). A machine that has never built a package has none, and
#    the failure lands after the kernels and every package have been fetched, so it is worth one
#    check here. `-a` writes the key where abuild looks for it, `-i` puts the public half in
#    /etc/apk/keys, where mkimage.sh copies keys from into the image, and `SUDO=` because this is
#    already running as root.
if ! ls "$HOME"/.abuild/*.rsa >/dev/null 2>&1; then
    echo "No abuild signing key; generating one"
    SUDO= abuild-keygen -a -i -n -q
fi

# 6. Hand over. These are what mkimg.morrow.sh and genapkovl-morrow.sh read; MORROW_REPO is how
#    both of them find the overlay and the package list from inside aports/scripts.
MORROW_REPO="$repo"
MORROW_HOSTNAME="morrow"
MORROW_BINARY="$binary"
MORROW_LEAN="$lean"
MORROW_FONT="$(morrow_resolve_font)"
# The name mkimage.sh builds the filename from. It appends "-${RELEASE}-${ARCH}.iso" itself, so this
# is a prefix and not a filename: an extension or an architecture here would appear twice in the end.
if [ "$lean" = 1 ]; then
    MORROW_IMAGE_NAME="${MORROW_IMAGE_NAME:-morrow-lean}"
else
    MORROW_IMAGE_NAME="${MORROW_IMAGE_NAME:-morrow}"
fi
export MORROW_REPO MORROW_HOSTNAME MORROW_BINARY MORROW_LEAN MORROW_FONT MORROW_IMAGE_NAME

mkdir -p "$outdir" "$work"
cd "$aports/scripts"
# --tag is the release the image calls itself, and it lands in the filename: $branch, not $tag, so
# that the ISO is morrow-3.20-x86_64.iso rather than morrow-v3.20-x86_64.iso. The repositories are
# the other way round -- the mirror has a directory called v3.20 -- and mkimage.sh has no --mirror
# option at all; it takes its package sources from --repository alone.
./mkimage.sh \
    --profile morrow \
    --arch "$arch" \
    --tag "$branch" \
    --outdir "$outdir" \
    --workdir "$work" \
    --repository "$mirror/$tag/main" \
    --repository "$mirror/$tag/community"

iso="$(ls -1t "$outdir"/*.iso 2>/dev/null | head -n 1 || true)"
[ -n "$iso" ] || die "the build finished without producing an ISO in $outdir"

cat <<EOF

ISO: $iso ($(du -h "$iso" | cut -f1))

Boot it with the memory this distribution is built for:

    qemu-system-x86_64 -m 512 -smp 2 -cdrom "$iso" -boot d

The 500 MB target is a claim about a running session, so measure it inside the session (free -m,
and morrow-watch's own MEMORY line) rather than estimating it from the build machine.
EOF
