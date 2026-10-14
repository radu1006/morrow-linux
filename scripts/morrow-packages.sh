# The Alpine packages Morrow needs, in one place.
#
# Two very different builds consume this file: `scripts/install-desktop.sh`, which turns an
# existing Alpine installation into the Morrow desktop, and `iso/profile/mkimg.morrow.sh`, which
# bakes the same desktop into the ISO. Keeping one list means the ISO and a hand-installed
# desktop cannot quietly drift apart, which is exactly the kind of difference that only shows up
# when a package is missing on a machine you cannot log into.
#
# The groups are not decoration: they fail differently. A missing X driver means no picture, a
# missing GL library means no window, and a missing font means a window full of blank boxes.

# The X server, its input driver, and udev so that a keyboard plugged in later is noticed.
MORROW_X_APKS="xorg-server xinit xauth eudev \
    xf86-input-libinput xf86-video-modesetting \
    xset xsetroot xkeyboard-config"

# What egui and eframe need at runtime. `mesa-dri-gallium` matters most: it brings llvmpipe, so
# a machine with no GL driver of its own still draws the window in software.
MORROW_GL_APKS="mesa-gl mesa-dri-gallium \
    libx11 libxext libxrandr libxcursor libxi libxcb libxkbcommon \
    fontconfig freetype"

# The desktop, deliberately small: a window manager, a panel, a file manager and a terminal.
# No session manager, no compositor, no desktop environment.
MORROW_DESKTOP_APKS="openbox tint2 pcmanfm xterm"

# Services. NetworkManager is what a laptop needs; dbus is what NetworkManager needs.
MORROW_SERVICE_APKS="dbus dbus-openrc networkmanager networkmanager-openrc"

MORROW_APKS="$MORROW_X_APKS $MORROW_GL_APKS $MORROW_DESKTOP_APKS $MORROW_SERVICE_APKS"

# Fonts, tried in order: the package was renamed in Alpine 3.19 (ttf-dejavu -> font-dejavu), and
# a package list that fails on one release is worse than a list that tries twice.
MORROW_FONT_APKS="font-dejavu ttf-dejavu"

# Services that must exist before anything is written to /dev, and those that can wait.
MORROW_SYSINIT_SERVICES="udev udev-trigger"
MORROW_DEFAULT_SERVICES="dbus NetworkManager"

# Extra packages needed only to *build* Morrow Watch on an Alpine machine, never installed into
# the image: rustup for a current musl toolchain, the C compiler eframe's dependencies need, and git.
MORROW_BUILDER_APKS="rustup build-base git ca-certificates curl"

morrow_setup_rust() {
    export PATH="$HOME/.cargo/bin:$PATH"
    if ! rustup toolchain list 2>/dev/null | grep -q '^stable-'; then
        rustup-init -y --default-toolchain stable --profile minimal
    fi
    export PATH="$HOME/.cargo/bin:$PATH"
}

# Install the first font package that exists on this release. Missing fonts are reported but do
# not stop the installation: a desktop without fonts is still a desktop you can fix.
morrow_install_fonts() {
    for font in $MORROW_FONT_APKS; do
        if apk add --no-cache "$font" >/dev/null 2>&1; then
            return 0
        fi
    done
    echo "Warning: no package in '$MORROW_FONT_APKS' was available; text may be unreadable." >&2
}

# Start the services the desktop depends on, skipping any this release does not ship.
morrow_enable_services() {
    for service in $MORROW_SYSINIT_SERVICES; do
        if [ -x "/etc/init.d/$service" ]; then
            rc-update add "$service" sysinit 2>/dev/null || true
        fi
    done
    for service in $MORROW_DEFAULT_SERVICES; do
        if [ -x "/etc/init.d/$service" ]; then
            rc-update add "$service" default 2>/dev/null || true
        fi
    done
}

# The first font package this machine's repositories actually have, printed and nothing else.
#
# Alpine renamed ttf-dejavu to font-dejavu in 3.19. The installer can simply try both, but the
# ISO builder bakes its package list into the image and fails at the end of a long build if a name
# does not exist there, so the driver asks the question first and passes the answer down.
morrow_resolve_font() {
    for font in $MORROW_FONT_APKS; do
        if [ -n "$(apk search -x -q "$font" 2>/dev/null)" ]; then
            printf '%s' "$font"
            return 0
        fi
    done
}
