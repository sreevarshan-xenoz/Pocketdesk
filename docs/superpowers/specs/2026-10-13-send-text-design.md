# Send text — design

Status: decided 2026-10-13, branch `feat/send-text`. Roadmap F2 ([feature roadmap](2026-10-12-feature-roadmap.md)). It builds on the in-session keyboard (F1).

## Why

The live keyboard (F1) types key by key as the IME edits. That is the right default, but three jobs need text sent as a whole:
- **Dictation and long text.** Voice typing and long sentences are easier to review before they land. KDE Connect's compose box exists for this reason, and its users ask for history.
- **Fields that block paste.** Some password fields, remote consoles and VM viewers ignore clipboard paste. Moonlight (Ctrl+Alt+Shift+V) and scrcpy (MOD+Shift+V) type the clipboard as keystrokes instead.
- **Resending.** The same command or address is often sent again.

## Decisions (made autonomously; each is cheap to revisit)

| # | Decision | Why |
|---|---|---|
| S1 | A **Send text** sheet: a multi-line field (the IME's voice key works in it), a **Send** button, a **Paste** button that fills the field from the phone's clipboard, and a "Press Enter after" switch. It opens from a ✎ cap on the keyboard bar and from Quick Settings. | One place for dictation, review and paste-as-typing. |
| S2 | Sending types the text through F1's ordered worker as one edit: text as text, `\n` as Enter, `\t` as Tab (the `KeyActions.forEdit` rules, with no modifiers). | One send path. The desktop sees exactly what live typing of the same text would produce. |
| S3 | A **Type phone clipboard** action in Quick Settings types the clipboard in one tap, without opening the sheet. | The Moonlight/scrcpy shortcut, for when paste is blocked. |
| S4 | **History**: the last 10 distinct texts sent, newest first, shown as chips in the sheet; a tap fills the field. It lives in memory only and is gone when the app's process ends. | Resending is the point. Keeping text the user may have typed into a password field on disk would be a leak waiting to happen. |
| S5 | Text over **4 000 characters** is refused with a message. | The server reads one input packet up to 64 KB, and typing a novel into a remote window is never what someone meant. |

## Out of scope

- Snippet libraries and macros (F9).
- Rich text.
- Sending files (F7).

## Testing

| Layer | Tests |
|---|---|
| JVM | `SendHistory`: newest first, de-duplicated, bounded at 10, blank text ignored. `TextSend`: Enter appended only when asked, and the 4 000-character limit. |
| Gates | `assembleDebug`, `lintDebug` (no new finding in changed files), `testDebugUnitTest`. |
| Device (only while the user is away from the desk; a scratch Notepad) | Send a two-line text and see both lines with the Enter between them. Paste then Send. Type phone clipboard. Pick a history chip. |
