#!/usr/bin/env sh
# Build the library the Android app loads (arm64-v8a) the same way everywhere:
# CI, releases and F-Droid's recipe run this, so their builds cannot drift.
#
# Pinned here: the Rust toolchain (rust-toolchain-version) and, for CI and
# F-Droid, the NDK (ndk-version, found through ANDROID_NDK_HOME). A developer
# whose default toolchain differs (on Windows, a GNU one) overrides the Rust
# pin with POCKETDESK_RUST_TOOLCHAIN.
#
# Reproducible: --locked dependencies, one codegen unit, stripped symbols,
# no build id, and the paths of this checkout, Cargo's home and the Rust
# toolchain mapped to fixed ones, so the library does not depend on where
# it was built.
set -eu

# Physical paths: rustc sees them, not a symlink's.
here=$(cd "$(dirname "$0")" && pwd -P)
root=$(cd "$here/../.." && pwd -P)
out="$here/../app/src/main/jniLibs"
toolchain=${POCKETDESK_RUST_TOOLCHAIN:-$(cat "$here/rust-toolchain-version")}
cargo_home=${CARGO_HOME:-$HOME/.cargo}

# The NDK: CI and F-Droid must use the pinned one (ANDROID_NDK_HOME); a
# developer may build with another, and is told so.
ndk=$(cat "$here/ndk-version")
case "${ANDROID_NDK_HOME:-}" in
    *"$ndk") ;;
    *)
        if [ "${CI:-}" = true ]; then
            echo "ANDROID_NDK_HOME must be the pinned NDK $ndk, not '${ANDROID_NDK_HOME:-}'" >&2
            exit 1
        fi
        echo "note: not the pinned NDK $ndk (ANDROID_NDK_HOME='${ANDROID_NDK_HOME:-}'): fine for development, not for a release" >&2
        ;;
esac

export RUSTUP_TOOLCHAIN="$toolchain"
# The toolchain's own sources (std, inlined into the library) live under it.
sysroot=$(rustc --print sysroot)

# On Windows (Git Bash) rustc sees these paths in Windows form.
if command -v cygpath >/dev/null 2>&1; then
    root=$(cygpath -w "$root")
    cargo_home=$(cygpath -w "$cargo_home")
    sysroot=$(cygpath -w "$sysroot")
fi

export CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1
export CARGO_PROFILE_RELEASE_STRIP=symbols
# rustc applies the last mapping that matches, so the checkout comes last:
# it wins even when it lies inside Cargo's home. Flags of the caller's go
# first, so they cannot undo these.
export RUSTFLAGS="${RUSTFLAGS:-} --remap-path-prefix=$cargo_home=/cargo --remap-path-prefix=$sysroot=/rust --remap-path-prefix=$root=/pocketdesk -C link-arg=-Wl,--build-id=none"

rm -rf "$out/arm64-v8a"
cd "$here"
# minSdk 26 (android/app/build.gradle.kts).
cargo ndk -t arm64-v8a --platform 26 -o "$out" build --release --locked
# cargo-ndk copies every library it built; the app loads only the bridge.
find "$out/arm64-v8a" -name '*.so' ! -name 'libpocketdesk_android_bridge.so' -delete
ls -l "$out/arm64-v8a"
