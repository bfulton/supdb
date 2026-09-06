#!/bin/sh
# Make libclang findable for bindgen, which the RocksDB comparator's build
# runs. Under GitHub Actions ($GITHUB_ENV set) it appends the variables to
# the job's environment; anywhere else it prints `export` lines to eval.
#
#   eval "$(sh scripts/libclang.sh)"        # locally
#   sh scripts/libclang.sh                  # in a workflow step
#
# The failure it prevents was two-shaped on the first CI run that built
# RocksDB at all. Linux: clang-sys searches for `libclang.so`, and the runner
# image ships only the versioned `libclang-18.so.1`, so nothing matched.
# macOS: the build script linked `@rpath/libclang.dylib` and dyld could not
# find it at run time -- SIGABRT before a line of C++ compiled. Naming the
# directory in LIBCLANG_PATH fixed Linux and not macOS: the link succeeds and
# the run still fails, because Apple's dylib carries an @rpath install name
# and the build script has no rpath for it. DYLD_LIBRARY_PATH cannot carry
# it either -- SIP strips DYLD_* from the environment whenever /bin/bash is
# exec'd, which is how every workflow step starts. So on macOS the script
# also adds the rpath to RUSTFLAGS, which cargo applies to build scripts.
set -eu

emit() {
  if [ -n "${GITHUB_ENV:-}" ]; then
    echo "$1=$2" >> "$GITHUB_ENV"
  else
    printf "export %s='%s'\n" "$1" "$2"
  fi
}

case "$(uname -s)" in
  Darwin)
    p="$(xcode-select -p)"
    # Full Xcode puts it under a toolchain; the Command Line Tools alone put
    # it directly under usr/lib. A self-hosted Mac may have either.
    for d in "$p/Toolchains/XcodeDefault.xctoolchain/usr/lib" "$p/usr/lib"; do
      if [ -f "$d/libclang.dylib" ]; then
        emit LIBCLANG_PATH "$d"
        emit RUSTFLAGS "${RUSTFLAGS:-}${RUSTFLAGS:+ }-C link-arg=-Wl,-rpath,$d"
        exit 0
      fi
    done
    echo "no libclang.dylib under $p" >&2
    exit 1
    ;;
  Linux)
    # Already findable: an unversioned .so somewhere ld looks, or the
    # variable set by whoever called us.
    if [ -n "${LIBCLANG_PATH:-}" ] && ls "$LIBCLANG_PATH"/libclang*.so >/dev/null 2>&1; then
      emit LIBCLANG_PATH "$LIBCLANG_PATH"
      exit 0
    fi
    for d in /usr/lib/llvm-*/lib /usr/lib /usr/lib64 /usr/lib/x86_64-linux-gnu /usr/lib/aarch64-linux-gnu; do
      if ls "$d"/libclang.so >/dev/null 2>&1 || ls "$d"/libclang-*.so >/dev/null 2>&1; then
        emit LIBCLANG_PATH "$d"
        exit 0
      fi
    done
    # Nothing unversioned anywhere. On a Debian-family host the package that
    # provides the symlink is libclang-dev; install it when we may.
    if command -v apt-get >/dev/null && command -v sudo >/dev/null; then
      sudo apt-get update -qq >/dev/null
      sudo apt-get install -y -qq --no-install-recommends libclang-dev >/dev/null
      d=$(ls -d /usr/lib/llvm-*/lib 2>/dev/null | sort -V | tail -1)
      if [ -n "$d" ] && ls "$d"/libclang.so >/dev/null 2>&1; then
        emit LIBCLANG_PATH "$d"
        exit 0
      fi
    fi
    echo "no libclang.so found; point LIBCLANG_PATH at a directory holding one" >&2
    exit 1
    ;;
  *)
    echo "unsupported OS: $(uname -s)" >&2
    exit 1
    ;;
esac
