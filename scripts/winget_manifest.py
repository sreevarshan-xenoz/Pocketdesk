#!/usr/bin/env python3
"""Write the winget manifests for a Pocketdesk release (C10).

Usage: python3 scripts/winget_manifest.py <version> <SHA256SUMS>
(`python` or `py` on Windows)

The release's SHA256SUMS (from its assets) gives the Windows zip's hash.
The three files go to packaging/winget/manifests/s/SreevarshanXenoz/
Pocketdesk/<version>/, the layout winget-pkgs uses, ready to validate with
`winget validate --manifest <that folder>` and to submit (the owner's call).
"""
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
ID = 'SreevarshanXenoz.Pocketdesk'
REPO = 'https://github.com/sreevarshan-xenoz/Pocketdesk'
SCHEMA = '1.10.0'


def manifests(version: str, sha256: str) -> dict:
    folder = f'pocketdesk-v{version}-windows-x86_64'
    zip_name = f'{folder}.zip'
    header = lambda kind: f'# yaml-language-server: $schema=https://aka.ms/winget-manifest.{kind}.{SCHEMA}.schema.json\n\n'
    return {
        f'{ID}.yaml': header('version') + f'''PackageIdentifier: {ID}
PackageVersion: {version}
DefaultLocale: en-US
ManifestType: version
ManifestVersion: {SCHEMA}
''',
        f'{ID}.installer.yaml': header('installer') + f'''PackageIdentifier: {ID}
PackageVersion: {version}
InstallerType: zip
NestedInstallerType: portable
NestedInstallerFiles:
- RelativeFilePath: {folder}\\pocketdesk.exe
  PortableCommandAlias: pocketdesk
- RelativeFilePath: {folder}\\pocketdesk-app.exe
  PortableCommandAlias: pocketdesk-app
# The server encodes video with ffmpeg on PATH; the zip does not carry it.
Dependencies:
  PackageDependencies:
  - PackageIdentifier: Gyan.FFmpeg
Installers:
- Architecture: x64
  InstallerUrl: {REPO}/releases/download/v{version}/{zip_name}
  InstallerSha256: {sha256.upper()}
ManifestType: installer
ManifestVersion: {SCHEMA}
''',
        f'{ID}.locale.en-US.yaml': header('defaultLocale') + f'''PackageIdentifier: {ID}
PackageVersion: {version}
PackageLocale: en-US
Publisher: sreevarshan-xenoz
PublisherUrl: https://github.com/sreevarshan-xenoz
PublisherSupportUrl: {REPO}/issues
PackageName: Pocketdesk
PackageUrl: {REPO}
License: MIT OR Apache-2.0
LicenseUrl: {REPO}/blob/v{version}/LICENSE-MIT
ShortDescription: Your Android phone and your computer, together - remote desktop, files, clipboard and notifications.
Description: |-
  Pocketdesk links an Android phone with this computer: see and drive the computer from the phone, send files and links both
  ways, share the clipboard, show the phone's notifications and texts here, and reach the computer from outside the home
  network without opening a port. Two programs: pocketdesk, the server, and pocketdesk-app, the desktop app to pair phones
  and see what is going on. After installing or upgrading, run `pocketdesk install` to start the server at logon and open its
  firewall rules (stop it with `pocketdesk stop` before an upgrade). FFmpeg, which encodes the video, comes as a dependency.
Tags:
- android
- clipboard
- file-transfer
- kde-connect
- phone
- remote-desktop
ReleaseNotesUrl: {REPO}/releases/tag/v{version}
ManifestType: defaultLocale
ManifestVersion: {SCHEMA}
''',
    }


def main() -> int:
    if len(sys.argv) != 3:
        print(__doc__, file=sys.stderr)
        return 2
    version, sums = sys.argv[1].lstrip('v'), Path(sys.argv[2])
    if not re.fullmatch(r'\d+\.\d+\.\d+', version):
        print(f'not a version: {version}', file=sys.stderr)
        return 2
    wanted = f'pocketdesk-v{version}-windows-x86_64.zip'
    # The file's name exactly (sha256sum may mark binary mode with a '*').
    sha = next(
        (parts[0] for parts in (line.split() for line in sums.read_text().splitlines())
         if len(parts) == 2 and parts[1].lstrip('*') == wanted),
        None,
    )
    if sha is None or not re.fullmatch(r'[0-9a-fA-F]{64}', sha):
        print(f'{wanted} is not in {sums}', file=sys.stderr)
        return 1
    out = ROOT / 'packaging' / 'winget' / 'manifests' / 's' / 'SreevarshanXenoz' / 'Pocketdesk' / version
    out.mkdir(parents=True, exist_ok=True)
    for name, text in manifests(version, sha).items():
        with open(out / name, 'w', encoding='utf-8', newline='\n') as f:
            f.write(text)
    print(f'wrote {out.relative_to(ROOT)}')
    return 0


if __name__ == '__main__':
    sys.exit(main())
