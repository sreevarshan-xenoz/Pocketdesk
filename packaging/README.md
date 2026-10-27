# Packaging

Ready-made entries for the places people install Pocketdesk from. Each one is published outside this repository, so sending it is the owner's decision; everything here is prepared and checked so that it is one step.

## Obtainium (the APK from the releases)

Nothing to submit. Obtainium follows the GitHub releases: each since 0.4.0 has exactly one APK, `pocketdesk-vX.Y.Z-android-arm64.apk`, which Obtainium picks without a filter. Add the app with `https://github.com/sreevarshan-xenoz/Pocketdesk`.

The APK is signed with the release key. Its certificate's SHA-256, for AppVerifier or `apksigner verify --print-certs`:

`3a2452b38b4be5ac8493b330a91ed5a43e8490e6c74b8a7934599bc5e8ac3a0f`

## The Debian/Ubuntu package

`packaging/linux/build-deb.sh <release> <binaries> <output>` builds `pocketdesk-<release>-linux-amd64.deb`. The release workflow runs it on Ubuntu after the release build. Its `Depends` come from `dpkg-shlibdeps`, so they are the libraries the binaries were linked to, and the package fits that Ubuntu release and newer ones that keep those libraries.

What it holds:
- `/usr/bin`: `pocketdesk`, `pocketdesk-app` and `pocketdesk-tray`.
- The systemd user unit, at `/usr/lib/systemd/user/pocketdesk.service` and pointed at `/usr/bin`.
- The menu entry, `dev.pocketdesk.Pocketdesk.desktop`.
- The icon, at 512 px and scalable.
- The man page, the docs and `copyright`.

What its scripts do:
- **Install:** `systemctl --global enable`s the unit, so the server starts at every user's sign-in, as Windows' logon task does.
- **Upgrade:** stops the servers of signed-in users and starts them again.
- **Remove:** stops them and disables the unit.
- **Home folders:** nothing in them is touched.

## winget (Windows)

`packaging/winget/manifests/s/SreevarshanXenoz/Pocketdesk/<version>/` holds the three manifests for a release, in the layout of [winget-pkgs](https://github.com/microsoft/winget-pkgs). They install the release's Windows zip as two portable commands, `pocketdesk` and `pocketdesk-app`, with FFmpeg (`Gyan.FFmpeg`) as a dependency, since the server encodes video with it. The server then wants `pocketdesk install` (logon start and firewall rules), and again after every upgrade, with `pocketdesk stop` first: the logon task and firewall rules name the version's folder, and a running server holds its file.

For each release:
1. `python3 scripts/winget_manifest.py <version> <the release's SHA256SUMS>` (`python` or `py` on Windows) writes the manifests.
2. `winget validate --manifest <that folder>` checks them.
3. To try them: `winget install --manifest <that folder>` (needs `winget settings --enable LocalManifestFiles` from an administrator).
4. To publish: a pull request to microsoft/winget-pkgs adding the folder (the owner's call; `wingetcreate submit` does it too).

## F-Droid

`packaging/fdroid/dev.pocketdesk.android.yml` is the draft of fdroiddata's metadata file: the bridge built as CI builds it (the pinned Rust, cargo-ndk, `android/bridge/build-android.sh`, JDK 21) and the release key's certificate, so F-Droid would ship the developer's signature on bytes it built itself. The Reproducible APK workflow shows two of CI's machines agree; F-Droid's build server (another JDK distribution, its own SDK) is not tried yet, and any difference fails its comparison. The store listing and screenshots are in `fastlane/metadata/android/en-US/`.

To publish: a merge request to fdroiddata adding the file as `metadata/dev.pocketdesk.android.yml` (the owner's call), with `fdroid lint` and `fdroid build` run on it there. Submit it with the first release that holds the screenshots: F-Droid reads the listing from the tag it builds, and the screenshots show settings newer than 0.4.0 (move the `Builds` entry to that release).

## Android developer verification

Registering the developer with Google for Android's developer verification (required from 2027 for apps installed outside Play) is the owner's decision; nothing here depends on it.
