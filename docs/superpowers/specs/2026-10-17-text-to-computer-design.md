# Phone → computer text in one tap (N2)

Date: 2026-10-17. Plan: `docs/research/2026-10-17-finish-and-polish.md` §N2.

## The problem

- **Sending text takes a detour.** Text shared from another app already goes to the computer's clipboard ("Send to Pocketdesk"), but selected text needs Copy, then the Quick Settings tile or the share sheet; Android 10+ lets no app read the clipboard in the background, so it cannot be automatic.
- **The share sheet lists Pocketdesk, not the computer.** With two saved computers, a share goes to the last one used; choosing another means opening the app.
- **A private copy can leave unasked.** The "Send clipboard" tile sends whatever is copied, even a copy the source app marked sensitive (a password manager's), which the computer side already keeps from phones the other way (T10b).

## The design

1. **Selected text → "To computer":** an activity alias in Android's text selection menu (`ACTION_PROCESS_TEXT`, `text/plain`), labelled "To computer". It takes `EXTRA_PROCESS_TEXT` and sends it to the last computer used's clipboard, as a shared text does, saying how it went. It never replaces the selection (it returns no result).
2. **Saved computers in the share sheet's Direct Share row:** each saved computer is a sharing shortcut (the launcher's "Connect to …" shortcuts gain the share category, through a `<share-target>` for `text/plain` and files). A share through one goes to that computer: the system share sheet puts the shortcut's id in `EXTRA_SHORTCUT_ID`, mapped to a saved computer only (an id naming none goes nowhere, never to another). This is the person's choice in the system's sheet, so it does not need "Let other apps choose a computer"; any app could forge the extra, but it can only pick among the owner's saved computers, which `ADDRESS` with the switch on allows already.
3. **A private copy asks first:** the tile reads the clip's description; a copy marked sensitive (`ClipDescription.EXTRA_IS_SENSITIVE`) is not sent until the person confirms in a small dialog ("Send a private copy?").

## Tests

JVM: which computer a share goes to with a shortcut id (a saved one, an unknown one, none), and the tile's check with a private copy.

## Device check

On the A71 against a throwaway server: select text in an app → "To computer" → the computer's clipboard (the stand-in file) holds it; a share through a computer's Direct Share target reaches that computer; a sensitive copy asks first.

## As built

- **"To computer":** the activity alias `.ToComputer` on `ShareActivity` with a `PROCESS_TEXT` filter; `ShareActivity` takes `EXTRA_PROCESS_TEXT` and sends it as a shared text (`sendText`, to the clipboard), returning no result.
- **Direct Share:** `res/xml/shortcuts.xml` (a `share-target` for `text/plain` and `*/*` with `ShortcutPlan.SHARE_CATEGORY`, on `MainActivity`); the launcher's connect shortcuts get the category and are long-lived (Android 11+). `ShareActivity` reads the shortcut id by its value (`Intent.EXTRA_SHORTCUT_ID` is API 29) and `ShortcutPlan.shortcutHost` (JVM-tested) maps `connect:<address>` to a saved computer, else nowhere.
- **A private copy:** `SendClipboard.check(…, private)` returns `AskPrivate` for a clip marked `android.content.extra.IS_SENSITIVE`; the tile's window then takes touches for an `AlertDialog` ("Send a private copy?") and sends on Send. Its messages name the computer (U3).
- **Checked on the A71** against a throwaway server with a stand-in clipboard file (checklist §92): Android lists "To computer" for selected text, and the selection menu's intent put the text on the computer's clipboard; the connect shortcuts carry the share category; a share through `connect:127.0.0.1` reached that computer, and one through an unknown id went nowhere.

