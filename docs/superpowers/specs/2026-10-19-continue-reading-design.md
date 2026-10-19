# D6, continue there (design and as built)

Date: 2026-10-19. Plan: `docs/research/2026-10-18-next-cycle.md`, D6.

## Why

Windows' Cross Device Resume (Insiders, August 2025) goes from the phone to the computer only (https://blogs.windows.com/windows-insider/2025/08/22/announcing-windows-11-insider-preview-build-26200-5761-dev-channel/). Pocketdesk already sends a page to the phone (ecosystem 1d), but the phone opens it at its top.

## What it does

- **The extension** has "Continue reading on phone" on the page's right-click menu. It reads the first six words of the first paragraph, heading, list item, quote or code block whose top is in the window (`chrome.scripting`, under `activeTab`: only the page right-clicked, only then).
- **The link** is sent with a text fragment for those words (`#:~:text=`; "-", "," and "&" encoded), so a browser that knows text fragments opens the page scrolled there. That is Chrome on Android, and Firefox 131 or later. Other browsers open it at its top.
- **Fallbacks:**
  - a fragment the page already had is replaced;
  - a link that would pass 2048 bytes goes as it was;
  - a page the browser does not let extensions read (its own pages, a store) goes as it is.
- **The rest** is the link path the menu's other items already take: native messaging, `pocketdesk browser-host`, and the phone's notification.

## Not done

- **What is playing.** Windows' media controls (GSMTC) and MPRIS name a title and an app, not a link, so there is nothing to open on the phone. Left out.
- **Phone to computer.** That is "Open on computer" in the phone's share sheet (T8a), which a browser's Share button reaches.

## Testing

- `continueUrl` was tried in Node: the encoding, a replaced fragment, an empty snippet and a link too long.
- The manifest test still holds.
- Owed (checklist §114): the menu in Chrome and Firefox, the phone's browser scrolling there, and a page that cannot be read.
