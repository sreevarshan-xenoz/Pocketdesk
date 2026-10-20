# H3: send the clipboard to the phone with a key (2026-10-20)

The phone sends text to the computer in one tap (N2). The computer sends its clipboard to the phone from the tray's menu (D5), two clicks away and a mouse's reach. A key pressed anywhere is quicker, so the computer gets one.

## What it does

**The setting:** desktop Settings "Send the clipboard with a key" (`clipboard_hotkey`, empty and off by default; Windows only). It takes a combination such as `win+shift+c`: Ctrl, Alt, Shift or Win, then a letter, a digit or F1 to F12. A key with no Ctrl, Alt or Win is refused, since it would take that key from every app.

**Registering it:** the tray program (`pocketdesk-tray`) registers it with `RegisterHotKey` on its own thread and catches `WM_HOTKEY` in the message pump it already runs.
- It reads the config again when the file changes.
- A config that does not read keeps the key as it is.
- A key another app holds is tried again every 30 s.

**A press:** sends what this computer has copied, as the tray's Send the clipboard does (D3's `request_current`, so a private copy or a pairing link never goes).
- **Which phone:** the one the clipboard reaches now, or the newest session's when several do.
- **A press queued before its key was taken back** is not acted on.

**What Settings says under the key:** what the tray made of it (`clipboard_hotkey.json`), since the tray has no console:
- registered;
- another app holds it;
- the last press found no phone;
- or not taken up yet, without the tray program.

The parser, the phone's choice and those words are in `desktop/src/hotkey.rs`, tested.

## The key in the tray (H4)

**In the tray's menu:** with the key registered, the tray program names it on the phone it sends to, as "Send the clipboard (Win+Shift+C)", so the key is found where its action is. This uses `tray_menu::with_key` and `hotkey::display`, both tested.

**Clicks and other trays:**
- The item's id stays the action's, so a click queued during a rebuild still finds it.
- The Linux tray never shows it, since the key is never registered there.

A fresh review found 0 Critical and 0 Important issues. Its one Minor, these docs, is done.

## Left out

- **Linux.** A global key there needs the GlobalShortcuts portal, so the row is not shown.
- **The window's own tray,** used without `pocketdesk-tray`, does not take the key.
- **Pinning the key to one phone.**

## Review

A fresh review found 0 Critical, 3 Important and 5 Minor issues. Each Important one was fixed in its own commit:
- **Failures went unseen,** because the tray has no console. Settings now shows the tray's status, and a taken key is retried.
- **A config that did not read unregistered the key.** It now keeps the key, and the config is read only when it changes.
- **The row showed on Linux** and did not say it needs the tray program. It now shows on Windows only, says so, and offers `win+shift+c`, since Ctrl+Alt is AltGr on many keyboards.

Minors fixed: a stale `WM_HOTKEY` after an unregister, and the parser's empty pieces tested.

Owed (by hand, with the owner): checklist §140.
