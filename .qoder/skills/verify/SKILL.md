---
name: verify
description: Run the full Rust quality gates (fmt, clippy in default and client feature profiles, workspace tests) and report what fails and why. Use before marking core/server/bridge work as done.
---

Verify current changes against the Pocketdesk quality gates.

1. Determine which files this session changed: `git status --short` and `git diff --name-only HEAD`.
2. Run, from the repo root, in order (each depends on the previous being meaningful, not on exit):
   - `cargo fmt --all -- --check` — pre-existing failures in files NOT touched this session are listed separately and do not count against the change; touched files must be rustfmt-clean (run `rustfmt --edition 2024 <file>` to fix).
   - `cargo clippy --workspace --all-targets -- -D warnings`
   - `cargo clippy -p pocketdesk-core --no-default-features --features client -- -D warnings`
   - `cargo test --workspace`
3. If any step fails, fix the failure if it comes from this session's changes, then re-run that step. Repeat until green or the failure is pre-existing debt.
4. Report a compact summary: each gate → PASS / FAIL / PASS-after-fix, plus any pre-existing (untouched-file) failures explicitly flagged as out of scope. Do not reformat or "fix" untouched files as part of verification.

If Kotlin/Gradle or bridge JNI files changed, the Android build IS verifiable on this host: build the bridge with `cd android/bridge && cargo ndk -t arm64-v8a -o ../app/src/main/jniLibs build`, then `cd android && ./gradlew assembleDebug` (SDK Platform 37 + NDK 29 configured; wrapper committed). On-device install/testing still requires a physical device or emulator.
