#!/bin/sh
# A filesystem that can encrypt, for the tests of a store under a key.
#
#   sudo sh scripts/fscrypt.sh up DIR [OWNER] [SIZE]   # make it and mount it
#   sudo sh scripts/fscrypt.sh down DIR                # unmount and remove it
#
# A store under a key (`Options::dir_key`) asks the kernel to encrypt its
# directory, which the kernel does on a filesystem made for it: ext4 or f2fs
# with the `encrypt` feature. A test cannot make one and most temporary
# directories are not on one, so this makes one in a file: DIR.img, an ext4
# with the feature on, mounted at DIR through a loop device. The file is
# what the tests read to see what the device holds, so it is left readable.
#
# `up` prints the two variables the tests want, one per line, as
#
#   SUPDB_FSCRYPT_DIR=...
#   SUPDB_FSCRYPT_IMAGE=...
#
# which is the form `eval "$(...)"; export ...` and a CI step's environment
# file both take. OWNER, as `uid:gid`, is who the mounted directory is given
# to: the tests run as that user and hand the kernel their keys as that
# user, which it allows of anyone for a key of this kind. SIZE is the
# file's, sparse, 512M unless given; a pairing of the suite's arms on the
# filesystem wants a few gigabytes.
#
# Needs root, for the mount. Fails, with what failed, where the kernel has
# no file encryption or no loop device.
set -u
cmd=${1:-}
dir=${2:-}
[ -n "$dir" ] || { echo "usage: fscrypt.sh up DIR [OWNER] [SIZE] | down DIR" >&2; exit 2; }
img="$dir.img"

case "$cmd" in
  up)
    owner=${3:-}
    size=${4:-512M}
    if mountpoint -q "$dir" 2>/dev/null; then
      echo "fscrypt.sh: $dir is mounted already; \`down\` it first" >&2
      exit 1
    fi
    mkdir -p "$dir" \
      && truncate -s "$size" "$img" \
      && mkfs.ext4 -q -F -O encrypt "$img" \
      && mount -o loop "$img" "$dir" \
      && chmod 644 "$img" \
      || { echo "fscrypt.sh: could not make an encrypting filesystem at $dir" >&2; exit 1; }
    if [ -n "$owner" ]; then
      chown "$owner" "$dir" || exit 1
    fi
    echo "SUPDB_FSCRYPT_DIR=$dir"
    echo "SUPDB_FSCRYPT_IMAGE=$img"
    ;;
  down)
    if mountpoint -q "$dir" 2>/dev/null; then
      umount "$dir" || { echo "fscrypt.sh: could not unmount $dir" >&2; exit 1; }
    fi
    rm -f "$img"
    rmdir "$dir" 2>/dev/null
    exit 0
    ;;
  *)
    echo "usage: fscrypt.sh up DIR [OWNER] [SIZE] | down DIR" >&2
    exit 2
    ;;
esac
