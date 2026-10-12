# In-session keyboard — design

Status: decided 2026-10-12, branch `feat/in-session-keyboard`. Roadmap F1 ([feature roadmap](2026-10-12-feature-roadmap.md)); evidence in [the research](../../research/2026-10-12-competitor-features.md).

## Why

During a session the phone cannot type. The bridge can already send key events and Unicode text (`RustCore.tapKey`, `hotkey`, `sendText`), and the Windows host injects text as UTF-16. But nothing captures the phone's keyboard, and there is no Ctrl, Alt or Esc outside the shortcut bar's fixed chips.

Success means:
- a person can tap a keyboard button, type into a desktop app with the phone's own keyboard (including swipe typing and autocorrect), and see each letter land as they type;
- they can press Ctrl/Alt/Shift/Super combinations, Esc, Tab, arrows and F-keys from a row above the keyboard;
- they never leave a modifier stuck down on the desktop.

## What exists

| Piece | State |
|---|---|
| Bridge | `send_keyboard_event`: a non-empty text sends `InputPacket::Text`. A key code sends `KeyEvent` press+release, or a press or release alone (the +50000 / +100000 offsets). Android `KEYCODE_*` values map to evdev in the bridge, within the `PHONE_EVDEV_KEY_RANGES` contract. |
| Server | Reads input streams one at a time, in order, so a press-tap-release combo arrives in order. Windows injects text via UTF-16 `SendInput`; Linux uinput `text()` has no Shift (roadmap 2666, roadmap F4). |
| App | `RustCore.tapKey/holdKey/releaseKey/hotkey/sendText`; a `ShortcutBar` of fixed chips; `LlIcons.Keyboard`; the activity is `adjustResize`. |

## Decisions (made autonomously; each is cheap to revisit)

| # | Decision | Why |
|---|---|---|
| K1 | A small focusable `KeyboardSinkView` (a plain `View`) is the IME target. Its `onCreateInputConnection` returns a `RemoteInputConnection`. The keyboard opens with `InputMethodManager.showSoftInput` and closes with `hideSoftInputFromWindow`. | The IME talks to the session directly, with no text field on screen. Every competitor that handles swipe typing well works at this level. |
| K2 | The connection keeps a `ShadowText` model: the text committed so far (a bounded tail) plus the current composing word, with the cursor always at the end. Each IME call (`setComposingText`, `commitText`, `finishComposingText`, `deleteSurroundingText`, `setComposingRegion` at the end) becomes an **edit**: backspaces to remove the differing suffix, then the new text. | Letters appear as they are typed, and swipe and autocorrect still work: the IME replacing a composing word becomes backspaces plus the new word. The model diffs only text the app itself sent, so it cannot drift the way RustDesk's whole-field diff does. |
| K3 | Backspaces are counted in **code points**. A `\n` in inserted text becomes an Enter key tap, and `\t` a Tab tap. | An emoji is two UTF-16 units but one Backspace on the desktop. |
| K4 | `getTextBeforeCursor` and `getCursorCapsMode` answer from the shadow model. | The IME then has real context, so sentence capitalisation and predictions behave as in a text field. |
| K5 | Modifiers latch **on the phone only**. Tap = the next key only, double-tap = locked until tapped again. A key or typed character while modifiers are active is sent as one `hotkey` combo: press the modifiers, tap the key, release in reverse. | The desktop never sees a held modifier, so none can get stuck (RustDesk's stuck-Shift bug), and a dropped connection mid-combo cannot leave one down. |
| K6 | Text typed through the IME while Ctrl, Alt or Super is active is sent as combos, one per character (a–z and 0–9 map to `KEYCODE_*`). The IME's state is then reset (`restartInput`) and the shadow cleared. Shift alone does not intercept text: the IME already sends the shifted character. | Ctrl latched plus "c" from Gboard must send Ctrl+C. Resetting stops the IME's next edit from deleting characters the desktop never received. |
| K7 | An accessory bar docks above the keyboard while it is open.<br>Row 1: Ctrl, Alt, Shift, Super (showing off / once / locked), Esc, Tab, the four arrows, and a hide-keyboard button.<br>Row 2 (scrolls): Home, End, PgUp, PgDn, Del, Ins, F1–F12, Ctrl+Alt+Del.<br>Bar keys respect the latched modifiers. | The Moonlight, RustDesk and Jump pattern users ask for. |
| K8 | A keyboard button sits in the session chrome next to the input-mode chip. Session chrome does not auto-hide while the keyboard is open. | A gesture alone is unreliable: phone makers' Android skins steal three-finger gestures. |
| K9 | `adjustResize` (already set) shrinks the session above the keyboard and bar, and the zoom state is kept. | The remote text cursor stays visible in every orientation, and zoom no longer resets when the keyboard opens (RustDesk #14286). |
| K10 | All sends run on one ordered worker: a single-threaded dispatcher queue. | The JNI calls block, and keys must arrive in the order they were typed. |
| K11 | Key events delivered to the sink view (`onKeyDown`, `onKeyUp`, used for hardware keys and `adb shell input`) go through the same path: printable characters as text, other keys as taps with the latched and held modifiers. | It costs a few lines, makes the feature testable from adb, and starts F9's hardware keyboard support. |

## Out of scope

- The compose box and "type the clipboard as keystrokes" (F2).
- Pointer capture and system-key capture for hardware keyboards (F9).
- User-defined macros.
- The Linux Unicode path (F4): on a Linux host, non-ASCII text depends on F4.

## Architecture

```
android/app  stream/keyboard/
  ShadowText.kt           composing/committed model → Edit(backspaces, text)   (pure)
  ModifierLatch.kt        off/once/locked per modifier; consume()               (pure)
  KeyActions.kt           Edit + modifiers → List<KeyAction>; char → KEYCODE    (pure)
  RemoteInputConnection.kt  BaseInputConnection glue → KeyActions → sender
  KeyboardSinkView.kt     focusable View: IME target + hardware key events
  KeyboardBar.kt          the accessory rows (Compose)
  KeySender.kt            ordered worker: KeyAction → RustCore calls
ui/RemoteScreen.kt        keyboard button, bar above the IME, chrome stays while open
```

`KeyAction` is one of `Text(s)`, `Tap(keyCode)` and `Combo(modifiers, keyCode)`. A run of backspaces is `Tap(KEYCODE_DEL)` repeated.

## Error handling

| Situation | Behavior |
|---|---|
| A send fails (link down) | Log it and drop the rest of that action. The session's own error card reports the link. |
| The IME calls something the model does not cover (selection in the middle of the text, a composing region not at the end) | Return true and change nothing. The next commit resyncs, because the model only ever appends at the end. |
| The keyboard is dismissed with Back | The bar hides through the IME-visibility inset, and the latches reset. |

## Testing

| Layer | Tests |
|---|---|
| JVM | `ShadowText`: typing a word with composing, an autocorrect replacement, commit after composing, deletes, emoji counted as code points, re-composing the last word, the bounded tail. `ModifierLatch`: tap, double-tap, consume. `KeyActions`: newline and tab become keys, combos for latched Ctrl, letters and digits mapped to `KEYCODE_*`. |
| Gates | `assembleDebug`, `lintDebug` (no new finding in changed files), `testDebugUnitTest`. |
| Device (Galaxy A71 → Windows) | With Notepad open on the desktop:<br>- type with Gboard and see the text appear as it is typed; swipe a word; let autocorrect replace a word;<br>- Ctrl latched + a, then Ctrl latched + c (select all, copy); read the Windows clipboard to compare what was typed;<br>- Esc, arrows and F-keys from the bar;<br>- `adb shell input text` (hardware-path) text;<br>- the zoom level survives the keyboard opening. |
