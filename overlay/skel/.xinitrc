#!/bin/sh
# Morrow's session: the file `startx` runs, and the whole of it.
#
# There is no display manager, no session manager and no compositor. On a 500 MB machine each of
# those is a resident process bought for a convenience nobody asked for, and the first two also
# mean a login screen that costs more memory than the desktop behind it.

# A flat root window costs nothing and cannot fail to load, which is more than a wallpaper daemon
# can say. The colour is the one the monitor paints its own background in, so the session looks
# deliberate rather than unconfigured.
xsetroot -solid '#121619' &

# The lean marker is a file rather than an environment variable so that the decision is made
# once, on disk, instead of being repeated in every script that wants to know.
if [ -e "$HOME/.config/morrow/lean" ]; then
    MORROW_LEAN=1
    export MORROW_LEAN
fi

exec openbox-session
