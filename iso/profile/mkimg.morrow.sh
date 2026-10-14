# Morrow's image profile for Alpine's own image builder.
#
# This is deliberately not a reimplementation of ISO building. Alpine ships `scripts/mkimage.sh`,
# upstream tests it, and it is the thing that knows about kernel flavours, initramfs features, the
# modloop, the bootloader menus and the four ways an x86 ISO can be booted. Morrow changes three
# things: what the machine calls itself, what /etc it boots with, and what is installed.
#
# The driver (iso/mkimage.sh) copies this file into aports/scripts/ beside Alpine's own profiles and
# runs the builder from there. mkimage.sh sources every mkimg.*.sh in that directory before it calls
# any of them, so profile_standard() and profile_base() are already defined by the time
# profile_morrow() runs.
#
# The declaration below has to stay exactly as it is, because mkimage.sh finds profiles by matching
# the line `^profile_(.*)() {$` against these files.
#
# The whole file runs inside mkimage.sh's shell, so the variables set here are read by the builder.

morrow_repo="${MORROW_REPO:-}"
if [ ! -f "$morrow_repo/scripts/morrow-packages.sh" ]; then
    echo "mkimg.morrow: MORROW_REPO must point at the Morrow checkout (got '$morrow_repo')" >&2
    exit 1
fi
# shellcheck source=scripts/morrow-packages.sh
. "$morrow_repo/scripts/morrow-packages.sh"

profile_morrow() {
    # Alpine's profile: the kernel, the initramfs, the modloop, the boot menus and the boot
    # repository are Alpine's work, not Morrow's. profile_standard() is in mkimg.standard.sh and
    # calls profile_base() from mkimg.base.sh, which is where alpine-base and the lts kernel come
    # from. If a future aports renames either, say so here rather than half-way through a build.
    if ! type profile_standard >/dev/null 2>&1; then
        echo "mkimg.morrow: aports no longer defines profile_standard(); see iso/README.md" >&2
        exit 1
    fi
    profile_standard

    # What makes the image Morrow. The hostname is also the name genapkovl-morrow.sh is handed as
    # its first argument and writes its archive under, so both come from the same variable.
    hostname="${MORROW_HOSTNAME:-morrow}"
    apkovl="genapkovl-morrow.sh"
    # A name, not a filename: mkimage.sh builds "${image_name}-${RELEASE}-${ARCH}.iso" from it, so
    # anything with an extension or an architecture in it here ends up in the name twice.
    image_name="${MORROW_IMAGE_NAME:-morrow}"

    # The desktop and the monitor come from the overlay, but the libraries they need have to be in
    # the image itself: an ISO that boots to a session which cannot load libGL is not a
    # distribution, it is an error message with a wallpaper.
    apks="$apks $MORROW_APKS${MORROW_FONT:+ $MORROW_FONT}"
}

