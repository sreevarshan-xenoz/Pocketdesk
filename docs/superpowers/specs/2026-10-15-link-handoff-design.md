# Link handoff (T8a)

Date: 2026-10-15. Status: design, on autopilot (the user asked for the roadmap to keep going; the decisions are mine).

## Why

T8 in the roadmap: KDE Connect's best-loved small features. Link handoff is the one people use without thinking: a page open on the phone, continued on the computer, or the other way round. Today Pocketdesk has none of it. A URL share was logged by the desktop, which then sent a "URL Received" notification back to the phone that shared it; the phone itself never sends one (shared text goes to the computer's clipboard).

## What

1. **Phone → computer.** The share sheet gets a second Pocketdesk entry, **Open on computer**, beside the existing one (which puts text on the computer's clipboard). A shared link goes to the last computer used (or the one an automation app names, as for any share), and the computer opens it in its default browser.
2. **Computer → phone.** The desktop app's Files tab gets **Send a link** (a field and a button), and the local API gets `pocketdesk link <phone> <url>`. The phone shows a notification, **Open link from <computer>**, that opens it in the phone's browser when tapped. (Android does not let an app open a browser from the background, and a link should not open itself on a phone in someone's pocket.)

## Safety

- **Only `http` and `https` links**, at most 2048 bytes, parsed as a URL on both ends; anything else (`file:`, `javascript:`, a custom scheme, a URL with credentials) is refused.
- **A new per-phone permission, Links**, on by default like Clipboard (a paired phone could already put the same link on the clipboard). The desktop's Devices tab has its switch.
- **A desktop setting, "Open links from phones"**, on by default; off, a link is shown as a notification instead of opened.
- **At most 5 links a minute per phone**, so a phone cannot open a browser storm.
- The computer opens a link with the system's handler for `http(s)` only: `ShellExecuteW` on Windows, `xdg-open` on Linux, never a shell.
- On the phone, **Settings → "Links from the computer"**, on by default; off, a link is dropped.

## Wire

- Both ways: `kdeconnect.pocketdesk.link` `{url}`, under the Links permission. Not KDE Connect's `share.request {url}`: the permission gate goes by packet type, and `share.request` is already under Files, so a link riding it would need both.
- The old `share.request {url}` branch on the desktop goes: no Pocketdesk phone sends it, and all it did was send a "URL Received" notification back to the phone that shared it.

## Tests

- core: the URL check (schemes, length, credentials, whitespace); `kdeconnect.pocketdesk.link` maps to Links.
- server: a link is opened (through a fake opener), refused by the check, by the permission, by the setting (shown instead), and by the rate limit.
- local API: `link` to a phone in a session queues the push; refusals with exit codes.
- bridge: a pushed link is kept for the app; the phone's send.
- Kotlin: the URL check, and the notification decision (setting off, not a link).
- Device: a link from Chrome's share sheet opening on Windows; one from the desktop app tapped on the phone.
