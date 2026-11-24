#!/bin/sh
# Make sure this host has a libclang bindgen can use, which the RocksDB
# comparator's build runs, and name the directory holding it. Under GitHub
# Actions ($GITHUB_ENV set) it appends LIBCLANG_PATH to the job's
# environment; anywhere else it prints an `export` line.
#
#   sh scripts/libclang.sh                  # here or in a workflow step
#
# bindgen loads libclang when RocksDB's build script runs (`bindgen-runtime`,
# Cargo.toml), and clang-sys then searches the usual directories by itself,
# so a build needs no variable: the script's work is to say whether the host
# has what that load needs, and to install it where it has not. Two things:
# the shared library, under any name the loader opens, and clang's own
# headers beside it (stdbool.h, stddef.h), since bindgen parses C with them
# and a host with the library alone ends at "'stdbool.h' file not found".
#
# Until the load was at run time the build script linked libclang, and that
# failure was two-shaped on the first CI run that built RocksDB at all.
# Linux: the linker wants an unversioned `libclang.so`, a -dev package's
# symlink, and the runner image, like a fresh container, has the versioned
# `libclang-18.so.1` of the runtime package alone; so this script installed
# libclang-dev with apt in every job, twenty seconds on a good day and five
# minutes on a slow mirror, a third of a megabyte of symlinks and three
# hundred megabytes of static libraries nothing read. macOS: the build
# script linked `@rpath/libclang.dylib` and dyld could not find it at run
# time, so the script put an rpath in RUSTFLAGS, DYLD_LIBRARY_PATH being
# stripped by SIP whenever /bin/bash is exec'd. Neither is needed by a
# library that is opened by its path.
set -eu

emit() {
  if [ -n "${GITHUB_ENV:-}" ]; then
    echo "$1=$2" >> "$GITHUB_ENV"
  else
    printf "export %s='%s'\n" "$1" "$2"
  fi
}

# A -dev package's name for the library, which comes with clang's headers.
unversioned() {
  for f in "$1"/libclang.so "$1"/libclang-[0-9]*.so; do
    [ -e "$f" ] && return 0
  done
  return 1
}
# The runtime package's. `libclang-cpp` is another library, the C++ one,
# and no name of it counts.
versioned() {
  for f in "$1"/libclang.so.[0-9]* "$1"/libclang-[0-9]*.so.[0-9]*; do
    [ -e "$f" ] && return 0
  done
  return 1
}
# clang's own headers: beside the library in an LLVM directory, or where the
# distribution links them for a library in a system directory. An LLVM
# directory answers for itself alone, since the system's link is some other
# version's.
headers() {
  for h in "$1"/clang/*/include/stddef.h; do
    [ -e "$h" ] && return 0
  done
  case "$1" in
    */llvm-*) return 1 ;;
  esac
  for h in /usr/lib/clang/*/include/stddef.h /usr/lib64/clang/*/include/stddef.h; do
    [ -e "$h" ] && return 0
  done
  return 1
}
usable() {
  unversioned "$1" || { versioned "$1" && headers "$1"; }
}

# The directories clang-sys searches on Linux, the LLVM ones first and the
# highest of them first: an image may carry several (Ubuntu 26.04's runner
# image has LLVM 20, 21 and 22), and the first version of the lookup after
# an install looked in the highest alone, found llvm-22 without the file it
# wanted, and failed with it one directory down.
find_dir() {
  for d in $(ls -d /usr/lib/llvm-*/lib 2>/dev/null | sort -rV) \
      /usr/lib /usr/lib64 /usr/lib/x86_64-linux-gnu /usr/lib/aarch64-linux-gnu; do
    if usable "$d"; then
      echo "$d"
      return 0
    fi
  done
  return 1
}

case "$(uname -s)" in
  Darwin)
    p="$(xcode-select -p)"
    # Full Xcode puts it under a toolchain; the Command Line Tools alone put
    # it directly under usr/lib. A self-hosted Mac may have either.
    for d in "$p/Toolchains/XcodeDefault.xctoolchain/usr/lib" "$p/usr/lib"; do
      if [ -f "$d/libclang.dylib" ]; then
        emit LIBCLANG_PATH "$d"
        exit 0
      fi
    done
    echo "no libclang.dylib under $p" >&2
    exit 1
    ;;
  Linux)
    # The directory whoever called us named, if it holds one.
    if [ -n "${LIBCLANG_PATH:-}" ] && usable "$LIBCLANG_PATH"; then
      emit LIBCLANG_PATH "$LIBCLANG_PATH"
      exit 0
    fi
    if d=$(find_dir); then
      emit LIBCLANG_PATH "$d"
      exit 0
    fi
    # No libclang with its headers anywhere. On a Debian-family host
    # libclang-dev brings both; install it where the host has apt, by the
    # engine's scripts/apt.sh.
    if command -v apt-get >/dev/null; then
      sh "$(dirname "$0")/../../scripts/apt.sh" libclang-dev
      if d=$(find_dir); then
        emit LIBCLANG_PATH "$d"
        exit 0
      fi
    fi
    echo "no libclang with clang's own headers found; point LIBCLANG_PATH at a directory holding one" >&2
    exit 1
    ;;
  *)
    echo "unsupported OS: $(uname -s)" >&2
    exit 1
    ;;
esac
