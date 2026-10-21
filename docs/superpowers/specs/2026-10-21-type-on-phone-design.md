# M4: type on the phone from the computer's keyboard

Plan: `docs/research/2026-10-21-m-cycle.md`, M4. This is part 1; part 2 is below under "Not now".

## Why

Writing a long message on the phone is easier with the computer's keyboard. C8 lets the computer type at the phone's field, but only during a cast, with the phone's screen in a window. KDE Connect's remote keyboard needs its own input method chosen on the phone first, and its users get stuck there (KDE bug 370919; KDE Discuss 2026-05).

## What

- **On the computer:** "Type on the phone" on each phone in the Overview opens a small window, "Typing to Ann's phone", which takes the computer's keyboard while it has focus.
  - **What it sends:**
    - text, typed at the phone's focused field;
    - Backspace and Enter, as keys;
    - Ctrl+V, as the clipboard's text, at most 1024 bytes as L2's paste;
    - its buttons Back, Home and Recents, as the phone's own.
  - **When it may:** only while the phone is linked, and only with the phone's "Show its screen or camera here" permission (which gates C8's control) and Settings "Control phones from their screen window" (C8's).
  - **Esc** closes it.
- **The road:** the window gathers what is typed and sends it about every 100 ms, as one batch per request in `typing_out/`. The server's watcher wakes on it and sends `kdeconnect.pocketdesk.type {events: [...]}` to that phone on its control link. Each event is one of:
  - `{"text": "..."}`;
  - `{"key": "backspace" | "enter"}`;
  - `{"global": "back" | "home" | "recents"}`.

  At most 64 events and 1024 bytes of text a batch, as C8's.
- **On the phone:** with "Let the computer control this phone" on and its Accessibility service on (C8), the notification listener runs a typing thread. It waits on the bridge's queue, with no poll, and does each batch through C8's service:
  - **Only from its own computer.**
  - **Never into a password field:** the computer sees nothing of the phone's screen, unlike during a cast. Text for one is dropped, and the notification says so.
  - **Never while the phone is locked:** nothing is typed then. Back and Home are as C8's.
  - **At C8's rate:** 200 events a second.
- **The notification:** while the computer types, an ongoing notification says "The computer is typing here", with Stop. It goes after 30 s without typing. Stop refuses typing for 10 minutes.

## Not now (part 2)

- Typing from the tray, and a key in desktop Settings (H3's way).
- Tab and the arrow keys: an Accessibility service cannot send keys, and moving the cursor needs the field's own selection, as C8's Backspace does.
- Linux's window: built and compiled, not run here.

## Privacy and safety

- **Typing into a screen nobody sees:**
  - the phone shows the notification while it happens;
  - the computer's window names the phone;
  - a password field takes nothing;
  - a locked phone takes nothing.
- **Advanced Protection** (Android 17) revokes the Accessibility service, as for C8, and the Health card says so (D1).

SECURITY.md limit 42.

## Tests

- **Core:** the batch's encoding and its limits.
- **Server:** a batch's packet; refused without the permission or the setting.
- **Phone (JVM):** `TypeRule`: own computer, the switch, the service, the lock, a password field, Stop's 10 minutes.
- **Desktop:** the window's events from egui's (text, Backspace, Enter, a paste cut to 1024 bytes).

## On the device (checklist §171)

- The OPPO with a scratch field in a throwaway app: text, a Backspace right after typing (C8's cached-field case), Enter, and Tamil, read back by `uiautomator dump`.
- A password field: nothing typed.
- The phone locked: nothing typed.
- Stop: refused for 10 minutes.
