# Release checklist

What has to be true before a `vX.Y.Z` (or `vX.Y.Z-beta.N`) tag is pushed. The tag starts `.github/workflows/release.yml`, which publishes at once, so every step here comes first. Tick them in the release's PR description, not in this file.

## 1. The tree

- [ ] `main` is green in CI on the commit to be tagged: `rust`, every `profiles` job, `windows`, `android`, and `audit` (or each audit finding written down in the release notes with why it stays).
- [ ] The version is the same in `Cargo.toml` (the workspace) and `android/app/build.gradle.kts` (`versionName`), and `versionCode` went up. The workflow refuses a tag that does not match them.
- [ ] `docs/capabilities.md` was regenerated on Linux and `docs/capabilities-windows.md` on Windows (`cargo run -q -p pocketdesk-server -- capabilities --markdown > <that file>`). `server/tests/capabilities_doc.rs` fails if either is stale for its own OS, but each OS checks only its own file.
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
- [ ] `pocketdesk --version` from each archive prints the tag's version.

## 4. On real machines

Run from the dry run's files, not from a dev build.

- [ ] **Linux:** unpack on a clean machine or a throwaway VM (see the checklist, §44, for the Azure recipe and its cost rules), start the server, pair the phone by QR, and open a session.
- [ ] **Windows:** unzip, `pocketdesk.exe install`, sign out and in, and check that the server came back. Pair and open a session.
- [ ] **Android:** install the APK over the previous release (an update, not a fresh install) and check that the saved computers and pairings survived.
- [ ] One session over the internet (iroh), from another network.

## 5. Publish

- [ ] Push the tag. When the workflow ends, open the release page and check the files and the notes once more.
- [ ] If anything is wrong, delete the release and the tag at once, fix it, and release the next patch version. Never move a published tag.
