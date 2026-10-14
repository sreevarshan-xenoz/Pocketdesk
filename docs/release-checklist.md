# Release checklist

What has to be true before a `vX.Y.Z` (or `vX.Y.Z-beta.N`) tag is pushed. The tag starts `.github/workflows/release.yml`, which publishes at once, so every step here comes first. Tick them in the release's PR description, not in this file.

## 1. The tree

- [ ] `main` is green in CI on the commit to be tagged: `rust`, every `profiles` job, `windows`, `android`, `bench` and `audit`. The release workflow audits again before it builds, so an advisory that must stay is ignored in `.cargo/audit.toml` with its reason, never left to fail.
- [ ] The version is the same in `Cargo.toml` (the workspace) and `android/app/build.gradle.kts` (`versionName`), and `versionCode` went up. The workflow refuses a tag that does not match them.
- [ ] `docs/capabilities.md` was regenerated on Linux and `docs/capabilities-windows.md` on Windows (`cargo run -q -p pocketdesk-server -- capabilities --markdown > <that file>`; on Windows from Git Bash, since PowerShell's `>` writes UTF-16 or a BOM). `server/tests/capabilities_doc.rs` fails if either is stale for its own OS, but each OS checks only its own file.
- [ ] `.github/release-notes.md` says what changed for a person using it, and lists what is **not** device-checked in this release (from the checklist's open items).

## 2. The docs

- [ ] README's "What works where" table matches this release: a new platform difference, or a row that is now device-checked, is in it.
- [ ] SECURITY.md's "Known limits" are still true, and any new one is there.
- [ ] AGENTS.md's Current Status has this release's milestones.
- [ ] Every checklist section the release's features add has its owed items listed, not silently skipped.

## 3. A dry run

- [ ] Actions → Release → Run workflow, on the commit to be tagged. Nothing is published. Download the `release-dry-run` artifact and check it holds:
  - the Linux tar.gz (server, desktop app, man page);
  - the Windows zip;
  - the APK, **release-signed** (`apksigner verify --print-certs`). Without the signing secrets the workflow falls back to the debug build, which must never ship as a release;
  - `SHA256SUMS`, matching each file.
- [ ] `pocketdesk --version` from each archive prints the version in `Cargo.toml` (the tag without its `v`, and without a `-beta.N` suffix).

## 4. On real machines

Run from the dry run's files, not from a dev build.

- [ ] **Linux:** unpack on a clean machine or a throwaway VM, start the server, pair the phone by QR, and open a session. (The R1 run used an Azure VM with a headless Sway; see the checklist, §44.)
- [ ] **Windows:** unzip, `pocketdesk.exe install`, sign out and in, and check that the server came back. Pair and open a session.
- [ ] **Android:** install the APK over the previous release, once there is one (an update, not a fresh install), and check that the saved computers and pairings survived.
- [ ] One session over the internet (iroh), from another network.

## 5. Publish

- [ ] Push the tag. When the workflow ends, open the release page and check the files and the notes once more.
- [ ] If anything is wrong, delete the release and the tag at once, fix it, and release the next patch version. Never move a published tag.
