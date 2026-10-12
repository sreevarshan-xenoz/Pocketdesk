# What makes a phone remote-desktop app good: competitor research

Researched 2026-10-12, for [the feature roadmap](../superpowers/specs/2026-10-12-feature-roadmap.md).

**Sources and limits.** This draws on:
- vendor documentation;
- the most-reacted GitHub issues of RustDesk, Moonlight, scrcpy, Sunshine, Apollo and GSConnect;
- Hacker News threads.

Reddit and Play Store review text could not be fetched, so the user evidence leans on GitHub and Hacker News. RustDesk (AGPL), Sunshine and Apollo (GPL) were studied for UX only, never for code. scrcpy (Apache-2.0) documentation was used for design patterns.

## What users ask for, with evidence

1. **Special keys and modifiers above the phone keyboard are the top input complaint.**
   - Moonlight issues ask for Ctrl, Super, Alt and Tab above the keyboard ([#975](https://github.com/moonlight-stream/moonlight-android/issues/975)), and report users going back to VNC Viewer for its keys ([#949](https://github.com/moonlight-stream/moonlight-android/issues/949)).
   - RustDesk's mobile client has latching Ctrl/Alt/Shift/Win toggles plus a scrollable "more" row: Esc, Tab, Home/End, Ins/Del, PgUp/PgDn, PrtScr, arrows, F-keys.
   - Jump Desktop puts arrows, F1–F12 and Ctrl+Alt+Del in a scrollable bottom toolbar ([changelog](https://changelog.jumpdesktop.com/fluid-2.0-in-browser-experience-uMERG)).
2. **How text is captured matters as much as the key row.**
   - KDE Connect sent key events, so swipe typing produced only the last letter of each word ([KDE bug 365305](https://bugs.kde.org/show_bug.cgi?id=365305)). It fixed this by composing text before sending ([1.19](https://www.apkmirror.com/apk/kde-community/kde-connect/kde-connect-1-19-1-release/)).
   - RustDesk diffs a hidden text field, which breaks on auto-paired brackets. Its users also report a stuck Shift and zoom resetting when the keyboard opens ([#14286](https://github.com/rustdesk/rustdesk/issues/14286)).
3. **The keyboard needs a visible button.** Several phone makers' Android skins (OEM ROMs) capture three-finger gestures, which breaks Moonlight's keyboard gesture ([#1229](https://github.com/moonlight-stream/moonlight-android/issues/1229)) and RustDesk's ([#3005](https://github.com/rustdesk/rustdesk/issues/3005)).
4. **Pinch-zoom and pan in every mode** are needed to read desktop text on a phone. See Moonlight [#1572](https://github.com/moonlight-stream/moonlight-android/issues/1572) and scrcpy [#24](https://github.com/Genymobile/scrcpy/issues/24) (45 reactions). Microsoft RD adds a pan control and shows it when the keyboard opens.
5. **Users want trackpad mode to behave like a laptop trackpad:**
   - two-finger right-click and horizontal scroll;
   - three-finger middle-click;
   - three-finger swipes for Alt+Tab and Win+Tab ([Moonlight #1312](https://github.com/moonlight-stream/moonlight-android/issues/1312));
   - sensitivity settings ([Artemis](https://github.com/ClassicOldSong/moonlight-android)).
6. **Discovery is a recurring failure point.**
   - KDE Connect "will fail to discover other devices unless I click refresh in both devices", while LocalSend "works every time" ([HN](https://news.ycombinator.com/item?id=45655852)).
   - mDNS breaks on machines with several network interfaces ([HN](https://news.ycombinator.com/item?id=45661121)).
   - RustDesk has an open request for LAN discovery ([#261](https://github.com/rustdesk/rustdesk/issues/261)).
7. **Image clipboard and file transfer are expected.**
   - RustDesk [#683](https://github.com/rustdesk/rustdesk/issues/683) asks for image clipboard.
   - Phone Link supports drag and drop of up to 100 files and 512 MB per file ([MS](https://support.microsoft.com/en-gb/windows/apps/phonelink/file-drag-drop-in-link-to-windows-pre-installed)).
   - RustDesk has a two-pane file manager ([docs](https://rustdesk.com/docs/en/manual/mobile)).
8. **Hosts expect to approve connections and set permissions.**
   - Parsec: approve in the app or with Ctrl+F1, and set per-guest permissions ([docs](https://support.parsec.app/hc/en-us/articles/4422936497165-Hosting-and-Permissions)).
   - RustDesk: access modes and per-permission toggles for keyboard, clipboard, files and audio, plus idle auto-disconnect ([docs](https://rustdesk.com/docs/en/self-host/client-configuration/advanced-settings/)).
9. **Hardware keyboards and mice on the phone** are a common power-user setup ([HN](https://news.ycombinator.com/item?id=32516789)). Android keeps Meta and Alt+Tab for itself ([Moonlight #1311](https://github.com/moonlight-stream/moonlight-android/issues/1311)).
10. **A virtual display sized to the phone** is a strong differentiator ([Apollo](https://github.com/ClassicOldSong/Apollo); a headless Hyprland output has 45 reactions on [Sunshine #2955](https://github.com/LizardByte/Sunshine/issues/2955)).

## How the best apps do mobile keyboard input

| Approach | Who | Strength | Weakness |
|---|---|---|---|
| Key-by-key events | Moonlight, KDE Connect (old), scrcpy `sdk` | Games, shortcuts, key repeat | Breaks swipe, autocorrect, dictation, non-Latin |
| Diff a hidden text field | RustDesk | Swipe and Unicode mostly work | Diff bugs, stuck Shift |
| Compose, then send | KDE Connect 1.19+ | Dictation, review before sending | Not live |
| Unicode / scancode toggle | Microsoft RD | User chooses per app | A buried setting |

scrcpy's split is the model: letters as key events (so shortcuts and WASD work), everything else as text ([doc](https://github.com/Genymobile/scrcpy/blob/master/doc/keyboard.md)).

The resulting design:
- The stream view is the IME target: `commitText` sends Unicode text; `deleteSurroundingText` sends Backspace; composing text waits for the commit.
- A docked accessory row has latching modifiers: tap latches for one key, double-tap locks.
- A second row holds the special keys.
- A persistent keyboard button opens it.
- The zoom level is kept when the keyboard opens.

## Desktop companion: what hosts offer

| Feature | Who |
|---|---|
| Approve-on-connect prompt (once / always / deny) | Parsec, RustDesk |
| Per-device permissions (view-only, input, clipboard, files, audio, power) | RustDesk, Apollo, Parsec |
| Visible active-session indicator, one-click disconnect | RustDesk connection manager |
| Discovery visibility toggle | requested in RustDesk #261 |
| Diagnostics: port check, interfaces, encoder, latency, copy debug report | Moonlight / Sunshine troubleshooting culture |
| Drop zone and transfer history | Phone Link, KDE Connect |
| Idle auto-disconnect; connect/disconnect hooks | RustDesk, Apollo |

## Ranked top 15 (user value × feasibility on Pocketdesk's architecture)

1. **In-session keyboard:** IME capture, latching modifier and special-key rows, a persistent button.
2. **Linux Unicode text path:** `zwp_virtual_keyboard_v1` with a keymap, plus the Shift fix (roadmap 2666).
3. **Pinch-zoom and pan in every mode,** keeping zoom when the keyboard opens.
4. **Complete trackpad gestures and sensitivity.**
5. **Approval prompt and per-device permissions** in the companion app.
6. **LAN discovery on the phone home screen,** with a hide toggle on the desktop.
7. **File transfer:** share sheet, remote browser, desktop drop zone.
8. **Type the clipboard as keystrokes, and a compose/dictation box.**
9. **Hardware keyboard/mouse passthrough with pointer capture.**
10. **Image clipboard both ways.**
11. **In-session monitor quick-switch.**
12. **Three-finger workspace and Alt+Tab swipes** (opt-in).
13. **Companion diagnostics and a copy-debug-report button.**
14. **Home-screen widget and Quick Settings tile** for connect and wake.
15. **Virtual display at the phone's resolution** (blocked: Hyprland's write IPC; Windows needs an indirect display driver).
