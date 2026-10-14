# Building the Morrow ISO

Morrow does not have its own image builder. Alpine ships `scripts/mkimage.sh` in the aports tree,
upstream tests it, and it is the thing that knows about kernel flavours, initramfs features, the
modloop and the several ways an x86 machine can be handed a Linux kernel. What this directory adds
is a profile on top of it, so that a change in how booting works arrives from upstream rather than
from a reconstructed copy of it here.

## What the three files are

| File | Role |
| --- | --- |
| `mkimage.sh` | The driver. Installs the build tools, clones aports, copies the profile in, builds the monitor as a musl binary, generates a signing key if the machine has none, then runs the builder. |
| `profile/mkimg.morrow.sh` | The profile the builder sources. Inherits Alpine's `profile_standard()` and changes the hostname, the package list, the image name and which overlay generator to use. |
| `profile/genapkovl-morrow.sh` | Writes `morrow.apkovl.tar.gz`: the `/etc` that is unpacked over a stock Alpine at boot. The session files, the desktop entry, X's input class, the runlevels, the package list and the monitor binary all travel in there. |

Both profile files are copied into `aports/scripts/` by the driver before the build, so they are read
from the same place, at the same revision, as Alpine's own profiles. `mkimage.sh` sources every
`mkimg.*.sh` in that directory before it calls any of them, which is how it finds `profile_morrow()`
— and how `profile_morrow()` finds `profile_standard()`.

Nothing Morrow-specific is written twice: the `/etc` that ends up in the archive is
`overlay/` from the repository root, and the package list is `scripts/morrow-packages.sh`. Those
are the same two things `scripts/install-desktop.sh` installs on a machine that already runs
Alpine, so the ISO and a hand-installed desktop are the same desktop.

## Requirements

- **Alpine, or an Alpine container.** The builder calls `apk --root`; a glibc distribution cannot
  host it, and a monitor built on one would not start on the machine the ISO is for.
- **Root**, because of that `apk --root`.
- **Network**, to clone aports and to fetch the packages the image is made of.
- Enough disk for a build tree: about 1 GB in `iso/` (aports plus the work directory).
- **Nothing installed beforehand.** The driver installs the toolchain itself — upstream's own list,
  from the comment at the top of `scripts/mkimage.sh`: `abuild` (which signs the image's index),
  `alpine-conf` (which builds the initramfs and the modloop), `fakeroot`, `syslinux`, `xorriso`,
  `mksquashfs`, `mtools` and `grub-efi` — plus `alpine-sdk`, and `rust`, `cargo` and `build-base`
  for the monitor.
- **An `abuild` signing key**, which the driver generates if the machine has none. The image carries
  a package repository of its own, the packages that came with the disc, and `abuild-sign` will not
  sign its index without a key. Without that step the build fails after the kernels and every
  package have been fetched, which is a long way in.

On a Windows or macOS host the first two of those have to come from somewhere else; see "Without an
Alpine machine" below for the two ways to get them.

## Build

```sh
MORROW_BRANCH=3.20 doas sh iso/mkimage.sh
```

That is the whole thing. The branch defaults to the release you are running, and everything has a
default that can be overridden:

| Variable | Default | Meaning |
| --- | --- | --- |
| `MORROW_BRANCH` | the running release | Which Alpine release and matching aports stable branch to build from (`3.20` uses `3.20-stable`; `edge` uses `edge`). |
| `MORROW_MIRROR` | `https://dl-cdn.alpinelinux.org/alpine` | Where packages come from. Point this at a local mirror for repeated builds. |
| `MORROW_ARCH` | `apk --print-arch` | Target architecture. |
| `MORROW_LEAN` | `0` | `1` writes the lean marker into `/etc/skel`, so new accounts get the monitor without the panel and the desktop icons. |
| `MORROW_BINARY` | `target/release/morrow-watch` | A binary to use instead of building one. |
| `MORROW_REBUILD` | `0` | `1` rebuilds even if a release binary is already there. |
| `MORROW_OUT` | `iso/dist` | Where the ISO is written. |
| `MORROW_WORK` | `iso/.cache` | Scratch space for the build. Keeping it between builds is what makes the second one quick. |
| `MORROW_IMAGE_NAME` | `morrow`, or `morrow-lean` under `MORROW_LEAN=1` | The name the ISO's filename is built from. |

`sh iso/mkimage.sh --skip-build` uses the existing binary, for when only the ISO needs rebuilding.
The exit is a single file in `iso/dist`, named `<MORROW_IMAGE_NAME>-<branch>-<arch>.iso`: the
`RELEASE` in mkimage.sh's `"${image_name}-${RELEASE}-${ARCH}.iso"` is the release the image calls
itself, and the driver passes the branch for it — 3.20 rather than v3.20, so the filename is
`morrow-3.20-x86_64.iso`.

### Without an Alpine machine

Neither Windows nor macOS can run the builder directly, because it needs `apk`. Two ways round it.

Both of them need a Linux VM underneath. On Windows that means the "Virtual Machine Platform"
optional component and virtualisation enabled in the firmware; without the pair, Docker Desktop
answers every command with `500 Internal Server Error` and `wsl --import` fails with
`HCS_E_HYPERV_NOT_INSTALLED`. Neither is a Morrow problem, and neither route gets round it: the
builder is a Linux program and needs a Linux kernel. From an Administrator prompt, then a reboot:

```powershell
wsl.exe --install --no-distribution
```

Virtualisation also has to be on in the firmware, usually called VT-x, AMD-V or SVM. Task Manager's
Performance tab reports it, under CPU, as "Virtualization: Enabled".

**In a container.** `iso/Dockerfile` exists for this. It installs `git` and nothing else, leaving the
toolchain to `iso/mkimage.sh`, so the build dependencies stay in one place:

```sh
docker build -f iso/Dockerfile -t morrow-builder .
docker run --rm --privileged \
    -v "$PWD/iso/dist:/out" \
    -v morrow-aports:/morrow/iso/aports \
    -v morrow-target:/morrow/target \
    -v morrow-cargo:/root/.cargo \
    morrow-builder
```

On Windows that last command is the same thing with `-v "${PWD}\iso\dist:/out"`.

`--privileged` is required rather than cautious: the builder chroots and mounts. The two named
volumes are optional, and their whole point is that a second build does not clone aports again or
recompile the monitor from nothing. Every variable in the table above goes through `-e`:

```sh
docker run --rm --privileged -e MORROW_LEAN=1 -e MORROW_BRANCH=edge \
  -v "$PWD/iso/dist:/out" -v morrow-aports:/morrow/iso/aports -v morrow-target:/morrow/target \
  -v morrow-cargo:/root/.cargo \
    morrow-builder
```

The repository is copied into the image rather than mounted, and `.dockerignore` is what keeps that
copy small. aports belongs in the container's filesystem, where git can check out its symlinks; on a
Windows bind mount they are not representable, and several hundred small files crawl.

**Under WSL2**, if you would rather have a real shell. The store has no Alpine, but WSL imports a
root filesystem happily:

```powershell
curl.exe -LO https://dl-cdn.alpinelinux.org/alpine/v3.20/releases/x86_64/alpine-minirootfs-3.20.10-x86_64.tar.gz
wsl --import MorrowBuild "$env:LOCALAPPDATA\MorrowBuild" .\alpine-minirootfs-3.20.10-x86_64.tar.gz
wsl -d MorrowBuild
```

Copy the repository into the WSL filesystem before building, and write the ISO back to the Windows
side, where QEMU can boot it:

```sh
cp -a /mnt/c/Users/you/Desktop/Linux ~/morrow
cd ~/morrow
MORROW_OUT=/mnt/c/Users/you/Desktop/Linux/iso/dist sh iso/mkimage.sh
```

Both routes end the same way: one `.iso` on disk, and the same `qemu-system-x86_64` line below.

## What the profile trusts about upstream

The profile is a small file on top of Alpine's, and the interfaces it depends on are worth naming.
All of these were read off `scripts/mkimage.sh`, `scripts/mkimg.base.sh` and `scripts/mkimg.standard.sh`
in the 3.20-stable tree:

- **`mkimage.sh` reads profiles from the directory it lives in**, by sourcing every `mkimg.*.sh` there
  and matching the line `^profile_(.*)() {$` to find their names. That is why `profile_morrow() {` is
  written exactly as it is, and why the driver copies the profile into `aports/scripts/` rather than
  passing a path. It also means `profile_standard()` and `profile_base()` are defined before
  `profile_morrow()` runs, which is what lets the profile inherit them; if a future aports renames
  either, `profile_morrow()` says so in one line instead of failing as `not found` mid-build.
- **The driver's grep is the same pattern**, so a rename fails in a second rather than at the end of a
  build that has already written a filesystem.
- **`mkimage.sh` executes the `apkovl` the profile names as `(cd $DESTDIR; fakeroot <script> <hostname>)`.**
  The hostname is `$1` and the archive has to be written into the current directory, which is the
  directory being assembled. `genapkovl-morrow.sh` is written against exactly that, and its header
  comment says so. This is also why the script must be executable: the driver installs it `0755`.
- **The image's package repository is signed.** `mkimg.base.sh`'s `build_apks` ends in `abuild-sign`,
  which exits with "No private key found" unless `PACKAGER_PRIVKEY` is set, so the driver generates a
  key with `abuild-keygen -a -i -n` when the machine has none. `-i` is not decoration either: keys
  are copied from `/etc/apk/keys` into the image, and the booted system needs the public half to use
  the repository on the disc.
- **`--tag` is a label, not a source.** It becomes `RELEASE`, which appears in the filename, in
  `/etc/alpine-release` and in the ISO's volume label; the package sources come from `--repository`
  alone, and `mkimage.sh` has no `--mirror` option at all.

Two things are still assumptions rather than checks, and both would fail loudly rather than quietly:
the `image_name`/`output_filename` defaults in `build_profile`, and the sections that `mkimg.base.sh`
defines (`kernels`, `apks`, `apkovl`, `syslinux*`, `grub*`, `iso`), which the builder drives by name
and which Morrow's profile does not touch.

## Verifying a build

```sh
qemu-system-x86_64 -m 512 -smp 2 -cdrom iso/dist/morrow-3.20-x86_64.iso -boot d
```

The 500 MB target is a claim about a running session, so it is worth measuring inside the session
rather than reasoning about it from the build machine:

- `free -m` for what the kernel, X, Openbox and the monitor actually add up to.
- `morrow-watch --text` for the same numbers as the monitor itself reports them.
- `ps` for the count: a session that is four processes is a session with nowhere to hide.

Boot with `MORROW_LEAN` built in to compare; the difference should be the panel and the desktop
icons, which are the two largest things in the session after X.

If QEMU is unavailable, write the ISO to USB and boot it on the target machine. The live session
can then be checked with `free -m`, `morrow-watch --text` and `ps`; the 500 MB target still needs
measurement on representative hardware.

## Status

Honest accounting. What has been checked here, on a Windows checkout with Git Bash:

- `sh -n` on every shell script in the repository.
- `genapkovl-morrow.sh` end to end, run the way the builder runs it — with the hostname as its
  argument, in the directory the archive belongs in — and with a stub in place of the monitor binary:
  the archive it wrote into that directory was unpacked and inspected. The session files are there
  with their modes, the lean marker appears only when it was asked for, `/etc/apk/world` holds the
  package list, and `overlay/`'s loopback file arrives as `/etc/network/interfaces`.
- `mkimage.sh --help`, an unknown argument (exit 2) and the refusal to run without `apk`.
- The driver and the profile against upstream's own scripts, read from the aports 3.20-stable tree
  rather than from memory. Four things were wrong before that reading, and all four are fixed: the
  profile sourced `mkimg.alpine.sh`, which does not exist — the profile worth inheriting is
  `profile_standard()`, in `mkimg.standard.sh`; the driver passed `--mirror`, which `mkimage.sh`
  rejects as an unknown option and would have exited on before doing anything; the package list was
  missing `abuild`, `alpine-conf`, `fakeroot` and `grub-efi`, all of which the builder shells out to;
  and `genapkovl-morrow.sh` read `$1` as an output directory, where the builder passes the hostname
  and expects the archive in the working directory. The last one is covered by the archive test above.
- The ISO was built in the Alpine 3.20 Docker container on Windows. The first attempt exposed two
  builder assumptions, now fixed: aports uses the `3.20-stable` branch, and its packaged Cargo 1.78
  is too old for the locked dependencies, so the build now bootstraps the stable musl toolchain with
  rustup. The resulting image is `iso/dist/morrow-3.20-x86_64.iso` (287 MiB; SHA-256
  `C70E7A234634E140EA3ED70A1E8C3C48B049BC099972563DE61C514E80385408`).
- ISO generation completed, but this host has no QEMU, so booting the image and measuring the desktop
  under the 500 MB / 2-core target remain unverified.
- **Line endings.** Every file in the tree is LF, and that was not true when an earlier version of
  this document said it was. The check used `grep`, and Git Bash's `grep` reads in text mode, where a
  carriage return cannot be seen; `tr -cd '\r' | wc -c` can. Nineteen files were CRLF, including
  `.xinitrc`, `autostart`, `menu.xml`, `/etc/network/interfaces` and the monitor's `.desktop` file —
  on Alpine each of those is broken by it, since `exec openbox-session\r` is a command that does not
  exist. They are converted, and `.gitattributes` asks git to hand out LF from now on.
- `ln -s` is the one step the archive test cannot exercise, since Windows will not make the runlevel
  symlinks here — which is why the script names the link it could not make. Those symlinks are the
  first thing to look at in the first Linux run.

The build itself has **not been run**, because it needs an Alpine host: the machine this was written
on has no `apk`, no `xorriso` and no `mksquashfs`, and it has no way to start a Linux VM either. Treat
the first build as a piece of work in its own right, but the parts of it that were guesses are guesses
about Alpine's own scripts, and those have now been read rather than remembered.
