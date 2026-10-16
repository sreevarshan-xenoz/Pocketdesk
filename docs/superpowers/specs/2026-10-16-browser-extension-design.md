# Send to phone from the browser (ecosystem phase 1d)

Part of the ecosystem plan (`docs/research/2026-10-16-ecosystem.md`, phase 1). The phone can already hand a link to the computer ("Open on computer") and the computer to the phone (the desktop app's Files tab, `pocketdesk link`). From inside the browser, where links and images are, it takes the most steps.

## What it does

- **A browser extension** (`browser/`, Manifest V3, the same files for Chromium browsers and Firefox) adds to the right-click menu:
  - "Send page to phone", or "Send link to phone" on a link: the phone gets it as `pocketdesk link` hands one over (a notification that opens it);
  - "Send image to phone" on an image: the image is downloaded by the computer's Pocketdesk, not the browser, and sent as a file, as `pocketdesk send` sends one.
- **Which phone:** for a link, any linked phone, as `pocketdesk link` takes them; for an image, the phone files reach now (a session first, else one linked at home), as "Send to phone" in the file manager picks. With several, the one most recently connected.
- **How it reaches Pocketdesk:** native messaging. The browser starts `pocketdesk browser-host`, registered by `pocketdesk browser-host --install` (and by Windows `install`, and the desktop app's Settings). There is no port and no listener, as with the rest of the local API. The host answers each request with `{ok}` or `{error}`, and the extension shows the answer as a notification.
- **Settings:** desktop Settings "Send from the browser" installs or removes the host registration, and says which browsers it found.

## The host

- **Protocol:** native messaging framing: a u32 length in native byte order, then UTF-8 JSON, at most 1 MiB in and out.
- **Requests:**
  - `{"send": "link", "url"}` → `{"ok": "sent to <phone>"}` or `{"error"}`;
  - `{"send": "image", "url"}` → the host downloads it with a cap of 32 MiB, `http(s)` only, never the computer's own loopback, a 20 s deadline, only PNG, JPEG, GIF, WebP, AVIF or BMP by its content type (not SVG, which can carry script), named from the URL's last part with the type's extension in place of the URL's own. It saves it to `pocketdesk-browser/` in the user's cache folder on Linux (never the shared `/tmp`) or the per-user temporary folder on Windows (owner-only; never the state folder, whose files the server does not send) and queues it through the outbox. `{"ok": "sent to <phone>"}` once it is queued.
- **Registration:**
  - Chromium (Chrome, Edge, Brave, Chromium, Vivaldi) by `allowed_origins` naming the extension's fixed id (a `key` in `manifest.json`);
  - Firefox by `allowed_extensions` naming `pocketdesk@pocketdesk.dev`;
  - on Windows, the registry keys `HKCU\Software\<vendor>\NativeMessagingHosts\dev.pocketdesk.host`;
  - on Linux, the browsers' `NativeMessagingHosts` / `native-messaging-hosts` folders.

## Checks

- **Rust:** the framing (lengths, caps, a short read); each request's answer; the URL rules for images (scheme, loopback, size); the manifest each browser gets; install and remove in a temp home or registry prefix.
- **Device:** the host driven by a script exactly as a browser drives it (link and image to the A71); the extension loaded unpacked in Chromium and Firefox (owed where this box has no such browser to drive).
