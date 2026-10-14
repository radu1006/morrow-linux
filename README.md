# Morrow Linux

Morrow is a lightweight Linux desktop built for old PCs. The target it is designed against is a
2-core machine with 500 MB of RAM — a budget that rules out a session manager, a compositor and a
desktop environment before a single application is started. Morrow pairs Alpine Linux with Openbox
and a small Rust system monitor, Morrow Watch, that reports what the machine is doing in the same
numbers the person watching it cares about.

## What's here

- **Morrow Watch** (`src/`): CPU load, memory, uptime, storage, network, thermal sensors and a
  searchable process list, sampled once a second. It runs as an egui window, as a text report, or
  as JSON — the same collector behind all three, so a report on a machine with no working GL
  driver says the same things as the window would.
- **The desktop itself** (`overlay/`): the session files Openbox needs — `.xinitrc`, `autostart`,
  `menu.xml`, X's input class and the monitor's desktop entry. Both install paths read this one
  directory, so an installed desktop and a booted ISO cannot drift apart.
- **Alpine desktop setup** (`scripts/install-desktop.sh`): turns an existing Alpine installation
  into the Morrow desktop. It installs packages, lays down the overlay, does not overwrite
  configuration it did not write, and refuses to run as anything but a non-root account.
- **A bootable ISO** (`iso/`): a driver around Alpine's own image builder, with a profile that
  changes the hostname, the package list and the overlay. See `iso/README.md`.
- **One package list** (`scripts/morrow-packages.sh`): the single source of truth for what Morrow
  is made of, read by the installer and by the ISO profile.
- **Lean release profile**: link-time optimisation and binary stripping, because the monitor runs
  beside the programs it is measuring.

## Build Morrow Watch

The target is musl Alpine, so the build that matters happens there:

```sh
apk add rustup build-base ca-certificates curl
rustup-init -y --default-toolchain stable --profile minimal
export PATH="$HOME/.cargo/bin:$PATH"
cargo build --release                # target/release/morrow-watch
```

A Windows or glibc build is a development artefact: it proves the monitor reads a machine
correctly, not that it can be shipped. `scripts/install-desktop.sh` and the ISO profile both say so
rather than silently copying a binary that will not start.

Run it from a session:

```sh
morrow-watch                         # the window
morrow-watch --text --width 96       # one report, for a terminal or a log
morrow-watch --json                  # the same numbers as a single JSON document
morrow-watch --text --interval 2     # live text, until interrupted
```

A build with the window opens it by default; a build without the `gui` feature prints instead of
complaining. `--interval` defaults `--count` to 0, which means live, and `--json` and `--gui` are for
one reading at a time: they refuse a rate or a count rather than accepting one they cannot honour, so
a refresh rate is asked for with `--text`. `--sort` and `--rows` choose what the process table shows,
`--colour` is only ever used when stdout is a terminal, and `--help` lists the rest.

## Set up an Alpine desktop

Build the release binary on the machine that will run it, then, as root on that machine:

```sh
MORROW_USER=your-login sh scripts/install-desktop.sh
```

Log in as that account and start the session with `startx`. The script installs the package list
from `scripts/morrow-packages.sh`, copies `overlay/` into place (leaving any file you have already
edited alone), enables dbus and NetworkManager, and puts the monitor in the session. It does not
partition disks or replace the installed system.

On a machine at or below 500 MB, add `MORROW_LEAN=1`: the panel and the desktop icons are the two
largest things in the session after X, and Openbox's own menu still reaches a terminal, the files
and the monitor without them.

## Build a bootable ISO

```sh
MORROW_BRANCH=3.20 doas sh iso/mkimage.sh
```

This runs on Alpine, or in an Alpine container: it drives Alpine's own image builder from a cloned
aports tree, so the kernel, initramfs, modloop and boot menus come from upstream rather than from a
copy of them kept here. The result is `iso/dist/morrow-<branch>-<arch>.iso`, and `iso/README.md` has
the requirements, the variables and the way to check a build.

On Windows or macOS there is no `apk`, so the build has to happen somewhere Alpine runs: a container
(`iso/Dockerfile`) or WSL2. `iso/README.md` has both, three commands each.

## Resource target

500 MB is a claim about a running session, so it belongs measured rather than asserted:

```sh
free -m                              # the whole session
morrow-watch --text                  # the same numbers as the monitor reports them
```

The choices that keep it plausible are all load-bearing rather than decorative: no display manager,
no session manager and no compositor, a window manager that costs a few megabytes, one colour on the
root window instead of a wallpaper daemon, software GL (llvmpipe) so a machine without a driver
still draws, and a monitor that samples once a second instead of spinning. The monitor's window
keeps a 90-sample history and two pages for the same reason.

What has been verified and what has not is worth stating plainly. The monitor's library has 58 unit
tests, builds clean under clippy with warnings denied on both feature sets, and its text, JSON and
error paths have been exercised end to end. All 58 pass on both feature sets too; one of them had
been asserting that `--interval` works without naming a front end, which a build with the window
does not allow — the test was corrected, not the behaviour.

The Alpine 3.20 ISO has now been generated in the Docker builder at
`iso/dist/morrow-3.20-x86_64.iso` (287 MiB). The build driver maps the release to aports' stable
branch and bootstraps the current musl Rust toolchain with rustup. This host has no QEMU, so the ISO
has not been booted here; measuring the live session on 2-core / 500 MB hardware remains ahead.
