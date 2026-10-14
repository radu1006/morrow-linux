# Build the Morrow ISO in a container.
#
# Alpine's image builder has to run on Alpine -- it is the same driver running here, and it calls
# `apk --root` and chroots into the tree it is assembling -- so on Windows or macOS the shortest
# route is a container. Build the image, then run it:

#     docker build -f iso/Dockerfile -t morrow-builder .
#     docker run --rm --privileged -v "${PWD}/iso/dist:/out" morrow-builder
#
# `--privileged` is not optional: the builder mounts and chroots. The rest of the toolchain
# (alpine-sdk, squashfs-tools, xorriso, mtools, cpio, rust, cargo) is installed by iso/mkimage.sh
# itself, so there is one list of build dependencies in the repository and this file is not a second
# copy of it.

FROM alpine:3.20

# git is here rather than in the driver so that the very first step -- cloning aports -- cannot fail
# on a container that has not been given the package list yet. The driver installs it too; apk is
# idempotent.
RUN apk add --no-cache git

# The repository is copied in, not bind-mounted. aports is cloned into iso/aports, and a Windows
# bind mount cannot hold the symlinks git checks out there; the same mount also makes a build of
# several hundred small files crawl. The only thing worth a mount is the output.
WORKDIR /morrow
COPY . /morrow

# Where the .iso lands. Mount a host directory here to keep it: the container's filesystem goes away
# with `docker run --rm`.
ENV MORROW_OUT=/out
VOLUME /out

CMD ["sh", "iso/mkimage.sh"]
