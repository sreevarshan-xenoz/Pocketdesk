#!/usr/bin/env bash
#
# Run the cargo-fuzz targets in fuzz/ (one per packet Pocketdesk parses).
#
# Usage: scripts/fuzz.sh [SECONDS] [TARGET...]
#
#   SECONDS   how long each target runs (default 30)
#   TARGET    the targets to run (default: every one `cargo fuzz list` names)
#
# Needs a nightly toolchain and cargo-fuzz (`cargo install cargo-fuzz`), on
# Linux with the server's build dependencies (docs/development-setup.md).
# FUZZ_TOOLCHAIN names the nightly (default `nightly`; CI pins a date, so
# its build cache lasts). FUZZ_SANITIZER picks libFuzzer's sanitizer: `none`
# by default (the parsers are safe Rust, so what a run finds is a panic or
# an overflow, with debug assertions on), or `address`.
#
# Each target's corpus grows in fuzz/corpus/<target>, started from the seeds
# in fuzz/seeds/<target>; a crash is written to fuzz/artifacts/<target>/ and
# ends the run with a failure.
set -euo pipefail

seconds="${1:-30}"
shift || true
sanitizer="${FUZZ_SANITIZER:-none}"
toolchain="${FUZZ_TOOLCHAIN:-nightly}"
# The toolchain's own host: a prebuilt cargo-fuzz (CI's is a static musl
# build) would otherwise build for the triple it was built for.
host="$(rustc +"$toolchain" -vV | sed -n 's/^host: //p')"
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root/fuzz"

# The workspace's lock, so the targets build the versions it pins.
cp "$root/Cargo.lock" Cargo.lock

targets=("$@")
if [ ${#targets[@]} -eq 0 ]; then
    mapfile -t targets < <(cargo +"$toolchain" fuzz list)
fi

cargo +"$toolchain" fuzz build --target "$host" --sanitizer "$sanitizer" --debug-assertions

# The longest input tried, per target: long enough to cross every limit it
# reads (a shared-input frame's 65,535 bytes, another computer's 256 KiB
# clipboard), and no longer, since a longer one slows every run.
max_len() {
    case "$1" in
        shared_input_wire) echo 70000 ;;
        computer_link | clipboard_push) echo 270000 ;;
        *) echo 20000 ;;
    esac
}

failed=()
for target in "${targets[@]}"; do
    mkdir -p "corpus/$target"
    echo "== $target: ${seconds}s"
    if ! cargo +"$toolchain" fuzz run --target "$host" --sanitizer "$sanitizer" --debug-assertions "$target" \
        "corpus/$target" "seeds/$target" -- \
        -max_total_time="$seconds" -max_len="$(max_len "$target")" -timeout=10 -print_final_stats=1; then
        failed+=("$target")
    fi
done

if [ ${#failed[@]} -gt 0 ]; then
    echo "Crashed: ${failed[*]} (inputs in fuzz/artifacts/)"
    exit 1
fi
echo "Every target ran ${seconds}s without a crash: ${targets[*]}"
