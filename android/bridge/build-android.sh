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

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
out="$here/../app/src/main/jniLibs"
toolchain=${POCKETDESK_RUST_TOOLCHAIN:-$(cat "$here/rust-toolchain-version")}
cargo_home=${CARGO_HOME:-$HOME/.cargo}

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
export RUSTFLAGS="--remap-path-prefix=$root=/pocketdesk --remap-path-prefix=$cargo_home=/cargo --remap-path-prefix=$sysroot=/rust -C link-arg=-Wl,--build-id=none ${RUSTFLAGS:-}"

rm -rf "$out/arm64-v8a"
cd "$here"
# minSdk 26 (android/app/build.gradle.kts).
cargo ndk -t arm64-v8a --platform 26 -o "$out" build --release --locked
# cargo-ndk copies every library it built; the app loads only the bridge.
find "$out/arm64-v8a" -name '*.so' ! -name 'libpocketdesk_android_bridge.so' -delete
ls -l "$out/arm64-v8a"
