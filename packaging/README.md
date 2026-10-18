# Packaging

Ready-made entries for the places people install Pocketdesk from. Each one is published outside this repository, so sending it is the owner's decision; everything here is prepared and checked so that it is one step.

## Obtainium (the APK from the releases)

Nothing to submit. Obtainium follows the GitHub releases: each has exactly one APK, `pocketdesk-vX.Y.Z-android-arm64.apk`, which Obtainium picks without a filter. Add the app with `https://github.com/sreevarshan-xenoz/Pocketdesk`.

The APK is signed with the release key. Its certificate's SHA-256, for AppVerifier or `apksigner verify --print-certs`:

`3a2452b38b4be5ac8493b330a91ed5a43e8490e6c74b8a7934599bc5e8ac3a0f`

## winget (Windows)

`packaging/winget/manifests/s/SreevarshanXenoz/Pocketdesk/<version>/` holds the three manifests for a release, in the layout of [winget-pkgs](https://github.com/microsoft/winget-pkgs). They install the release's Windows zip as two portable commands, `pocketdesk` and `pocketdesk-app`; the server then wants `pocketdesk install` once (logon start and firewall rules).

For each release:
1. `python3 scripts/winget_manifest.py <version> <the release's SHA256SUMS>` writes the manifests.
2. `winget validate --manifest <that folder>` checks them.
3. To try them: `winget install --manifest <that folder>` (needs `winget settings --enable LocalManifestFiles` from an administrator).
4. To publish: a pull request to microsoft/winget-pkgs adding the folder (the owner's call; `wingetcreate submit` does it too).

## F-Droid

`packaging/fdroid/dev.pocketdesk.android.yml` is the draft of fdroiddata's metadata file: the build as CI does it (the pinned Rust, cargo-ndk, `android/bridge/build-android.sh`) and the release key's certificate, so F-Droid ships the developer's signature on bytes it built itself (the Reproducible APK workflow checks two machines agree). The store listing and screenshots are in `fastlane/metadata/android/en-US/`.

To publish: a merge request to fdroiddata adding the file as `metadata/dev.pocketdesk.android.yml` (the owner's call), with `fdroid lint` and `fdroid build` run on it there.

## Android developer verification

Registering the developer with Google for Android's developer verification (required from 2027 for apps installed outside Play) is the owner's decision; nothing here depends on it.
