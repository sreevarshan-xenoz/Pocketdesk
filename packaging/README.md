# Packaging

Ready-made entries for the places people install Pocketdesk from. Each one is published outside this repository, so sending it is the owner's decision; everything here is prepared and checked so that it is one step.

## Obtainium (the APK from the releases)

Nothing to submit. Obtainium follows the GitHub releases: each since 0.4.0 has exactly one APK, `pocketdesk-vX.Y.Z-android-arm64.apk`, which Obtainium picks without a filter. Add the app with `https://github.com/sreevarshan-xenoz/Pocketdesk`.

The APK is signed with the release key. Its certificate's SHA-256, for AppVerifier or `apksigner verify --print-certs`:

`3a2452b38b4be5ac8493b330a91ed5a43e8490e6c74b8a7934599bc5e8ac3a0f`

## The Windows installer

`packaging/windows/pocketdesk.iss` is the Inno Setup 6 script for `pocketdesk-<version>-windows-x86_64-setup.exe`. The release workflow builds it beside the zip.

**What it does:**
- Shows what Pocketdesk sends to whom before installing (`privacy.txt`, a short form of `docs/privacy.md`).
- Installs to Program Files and adds Pocketdesk to the Start menu and to Apps & features.
- Offers three tasks:
  - start at sign-in: `firewall-add` elevated, then `install --no-firewall` as the user who ran it;
  - `pocketdesk` on the system `PATH`;
  - FFmpeg through winget, offered only when it is missing.
- On an upgrade, stops the running server, the tray and the app first.

**The uninstaller:**
- Stops them, then runs `pocketdesk uninstall` (the logon task, the firewall rules, Send To and the browser host).
- Takes `pocketdesk` back off `PATH`.
- Asks before deleting `%APPDATA%\pocketdesk` and `%LOCALAPPDATA%\pocketdesk`. A silent uninstall keeps them.

**Failures:** a failed step is reported in a message box and in the setup log. A silent install only logs it.

**Silent installs:** `/VERYSILENT` runs every task unless `/TASKS=` names some.

**Admin account:** the per-user parts follow the user who started setup: the logon task, Send To, the browser host, and the folders the uninstaller asks about. If a standard user enters another administrator's credentials, the uninstaller's per-user clean-up applies to that administrator instead.

`packaging/windows/smoke-test.ps1 <setup.exe>` installs it silently, checks the files, Start menu entry, Apps & features entry, PATH, logon task and firewall rules, then uninstalls it and checks that each went. The release workflow runs it on its runner after every build. Run it only on a throwaway machine.

To build it locally, run this from the repository root after `cargo build --release`:

```
ISCC packaging\windows\pocketdesk.iss /DAppVersion=v0.7.0 /DNumericVersion=0.7.0
```

The output goes to `release\`. `/DSourceDir=` takes the executables from another folder. The wizard's images (`wizard-*.bmp`) are rendered from `assets/brand/pocketdesk-mark-512.png`, and the icon from `pocketdesk-icon-1024.png`.

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

`packaging/linux/smoke-test.sh <the .deb>` installs it with apt, checks the programs, unit, menu entry, icon and man page, removes it, and checks that each went. The release workflow runs it on its runner after every build.

## winget (Windows)

`packaging/winget/manifests/s/SreevarshanXenoz/Pocketdesk/<version>/` holds the three manifests for a release, in the layout of [winget-pkgs](https://github.com/microsoft/winget-pkgs). They install the release's Windows zip as two portable commands, `pocketdesk` and `pocketdesk-app`, with FFmpeg (`Gyan.FFmpeg`) as a dependency, since the server encodes video with it. The server then wants `pocketdesk install` (logon start and firewall rules), and again after every upgrade, with `pocketdesk stop` first: the logon task and firewall rules name the version's folder, and a running server holds its file.

For each release:
1. `python3 scripts/winget_manifest.py <version> <the release's SHA256SUMS>` (`python` or `py` on Windows) writes the manifests.
2. `winget validate --manifest <that folder>` checks them.
3. To try them: `winget install --manifest <that folder>` (needs `winget settings --enable LocalManifestFiles` from an administrator).
4. To publish: a pull request to microsoft/winget-pkgs adding the folder (the owner's call; `wingetcreate submit` does it too).

## F-Droid

`packaging/fdroid/dev.pocketdesk.android.yml` is the draft of fdroiddata's metadata file: the bridge built as CI builds it (the pinned Rust through Debian's `rustup`, cargo-ndk, `android/bridge/build-android.sh`, the build server's own JDK) and the release key's certificate, so F-Droid would ship the developer's signature on bytes it built itself. The Reproducible APK workflow shows two of CI's machines agree; F-Droid's build server (another JDK distribution, its own SDK) is not tried yet, and any difference fails its comparison. The store listing and screenshots are in `fastlane/metadata/android/en-US/`.

Submitted on 2026-10-28 as [fdroid/fdroiddata!54623](https://gitlab.com/fdroid/fdroiddata/-/merge_requests/54623); its review asked for rustup from Debian and no JDK install, as this file now has, and its first CI run moved the Rust build from `prebuild` to `build` (the scan runs between them and refused the fresh `target/`) and wanted fdroidserver master's formatting (`fdroid rewritemeta` wraps `Binaries` and `AllowedAPKSigningKeys`; the comments here are dropped there). To publish another way or again (the owner's call, from a GitLab account):
1. Fork [fdroiddata](https://gitlab.com/fdroid/fdroiddata) and add the file as `metadata/dev.pocketdesk.android.yml`. It carries one `Builds` entry, the latest release (0.8.2): fdroiddata keeps only the newest build of a new app, so each release replaces the entry rather than adding one (0.7.0 was the first whose tag holds the screenshots F-Droid reads from `fastlane/`).
2. In the fork, `fdroid lint dev.pocketdesk.android` and `fdroid rewritemeta dev.pocketdesk.android`. Run against this file outside fdroiddata (fdroidserver from pip, 2026-10-28), lint flagged only the categories, which a checkout without fdroiddata's `config/categories.yml` cannot know (all three are in it), and rewritemeta changed nothing but dropping the comments, which fdroiddata's copy goes without. fdroiddata's CI runs `fdroid build` on the merge request.
3. Open the merge request with fdroiddata's *App inclusion* template.

After that, F-Droid's update check (`UpdateCheckMode: Tags`) picks up each new `vX.Y.Z` tag by itself.

## AUR (Arch Linux)

`aur/PKGBUILD` builds the release's source tarball as the release workflow does (the server, then the desktop app and the tray), and installs the three programs, the systemd user unit pointed at `/usr/bin`, the menu entry, the icon, the man page and the licences. Tailscale is optional, not a dependency. It is published on the AUR as [`pocketdesk`](https://aur.archlinux.org/packages/pocketdesk) (maintainer `xenoz`, from 0.7.0).

For each release, once its tag is pushed:
1. In `aur/`, set `pkgver`, run `updpkgsums` (it replaces `SKIP` with the tarball's checksum) and `makepkg --printsrcinfo > .SRCINFO`.
2. `makepkg -si` on an Arch machine or in an `archlinux` container to check it builds and installs.
3. To publish (the owner's AUR account, with its SSH key and a verified e-mail address, which the AUR requires before a push): in a clone of `ssh://aur@aur.archlinux.org/pocketdesk.git`, copy in `PKGBUILD`, `pocketdesk.install` and `.SRCINFO`, commit and push to `master`. Commit the same `aur/PKGBUILD` here, so the two stay identical.

## Signing the Windows programs

The Windows programs and `setup.exe` are unsigned, so SmartScreen calls their publisher unknown. [SignPath Foundation](https://signpath.org/) signs open-source projects for free; the certificate is issued to SignPath Foundation, which then shows as the publisher. Applying is the owner's call:
1. Apply at [signpath.io/product/open-source](https://about.signpath.io/product/open-source). Every member with access needs multi-factor authentication on GitHub and SignPath.
2. Publish a code signing policy page (who builds and approves releases; the line *Free code signing provided by SignPath.io, certificate by SignPath Foundation*) on the landing page or in the README.
3. Once accepted, add the API token as a repository secret, and a step in the release workflow's Windows job that sends `pocketdesk.exe`, `pocketdesk-app.exe`, `pocketdesk-tray.exe` and the setup to SignPath (`signpath/github-action-submit-signing-request`) before the zip, the installer and `SHA256SUMS` are made. SignPath checks that the files came from that workflow run (origin verification).
