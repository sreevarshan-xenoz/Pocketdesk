# Answer a phone notification in the toast itself (N3)

Date: 2026-10-17. Plan: `docs/research/2026-10-17-finish-and-polish.md` §N3. Builds on T10c (answers from the desktop app's Notifications page).

## The problem

A phone notification that offers buttons or a reply (T10c: the phone's "Let the computer answer notifications" on, sent in full) shows on the computer as a plain toast. To answer it the owner opens the desktop app's Notifications page. Phone Link answers in the toast; GSConnect did until GNOME 45.

## The design

1. **Windows:** a phone notification the server keeps to answer (T10c's `PhoneNote`) shows as a toast with its buttons (up to 3) and, when it takes one, a reply box with Send. The XML is built by a pure function (`toast_xml`), tested: titles and placeholder escaped, at most 5 actions (Windows' limit; the phone sends at most 3 buttons and one reply).
2. **The answer** goes the way the Notifications page's does: the toast's `Activated` event (in the server's process, which runs while toasts show) reads the action's arguments (`reply`, or `action:<n>` into the buttons the toast was shown with) and the reply text, and writes an answer request with `phone_notes::request`. The server's answer loop then holds it to the same rules: only what the notification still offers, the phone's Notifications permission, the phone in reach within a minute, at most 10 a minute on the phone. Parsing the activation is a pure function (`toast_answer`), tested.
3. **Kept alive:** the toast objects of answerable notifications are held (the newest 30, the same bound as the kept notes), so their `Activated` handler lives; removing a toast (the phone closed it) drops it.
4. **Linux:** the D-Bus notification carries the buttons as actions (`a0`…`a2`, titled) and, on servers that offer it (KDE's `inline-reply`), a reply; the server listens for `ActionInvoked` and `NotificationReplied` on its own notifications and writes the same request. Servers without actions show the plain notification.
5. **Settings:** desktop Settings, Notifications and texts, "Answer in the notification" (`toast_answers`, on by default; read at each notification, so it applies at once). Off, the toast is plain and the Notifications page still answers.

## Limits

- An unpackaged Windows app gets the `Activated` event only while its process runs and the toast object lives; answering from the Action Center after the server restarted does nothing (the Notifications page still answers). Reports differ on whether `Activated` fires from the Action Center at all for an app with no COM activator, and a server installed to run elevated may behave differently: unchecked until a press is tried (checklist §96). A COM activator, which would cover all of it, needs a registered class: not now.
- The reply box text is the owner's; it is written owner-only, as the page's is.

## Tests

`toast_xml` (no actions, buttons, reply, escaping, the action cap) and `toast_answer` (reply with text, an empty reply refused, a button by index, an index out of range, unknown arguments). A live `#[ignore]`d test shows an answerable toast and checks Windows accepts the XML.

## Check

On this box with the A71 and a throwaway chat app (T10c's): an answerable notification shows its buttons and reply box (read through UI Automation, never clicked). Clicking is owed to the owner.

## As built

- **`server/src/toast_answers.rs`** (pure, tested on every OS): `Answerable` (whose, the buttons in the order shown, whether it takes a reply), `toast_xml` (the reply box with Send first, then the buttons, at most five actions in all, titles escaped), `toast_answer` (`reply` with non-empty text, or `action:<n>` into the buttons the toast showed), `dbus_actions`, `dbus_action_answer`, `dbus_reply_answer`, and `send`, which writes the answer with `phone_notes::request` as the Notifications page does.
- **Windows** (`platform/windows.rs`): `show_toast` builds the XML from `toast_xml`; for an answerable phone toast it registers `Activated` (reading the arguments and the reply box from `ToastActivatedEventArgs.UserInput`) and keeps the toast in `ANSWERABLE` (the newest 30, dropped by `remove_tagged`). The `windows` crate gains `Foundation_Collections` for the reply's `ValueSet`.
- **Linux** (`plugins/notification.rs`): an answerable one's `Notify` carries `a0`… and `inline-reply` with the `x-kde-reply-placeholder-text` hint, and stays 20 s; `listen_for_answers` starts one task for the server's life that takes `ActionInvoked` and `NotificationReplied` for the ids shown as answerable. Type-checked here against zbus 5 in a scratch crate (the lockfile has 5.15); built and tested in CI.
- **Settings**: `toast_answers` (config, desktop Settings "Answer in the notification" under Notifications and texts), read through `current_config` at each notification, so it applies at once.
- **Checked on this box with the A71**: a throwaway chat app (not committed) posting "Reply" and "Mark as read"; the phone forwarded it, the server kept it answerable and showed the toast, whose delivered XML in the notification history held the reply box, Send and "Mark as read"; clearing it on the phone took the toast back. Windows held the banner (a full-screen video was playing), so the toast was read from history, never pressed.

## Review

A fresh review (0 Critical, 2 Important, 4 Minor), fixed one commit each:

- **A dead Reply button on Linux servers without KDE's inline reply** (GNOME draws every action as a button): the server's `GetCapabilities` is asked once, buttons only where it lists `actions`, the reply only where it lists `inline-reply`.
- **SECURITY.md, docs/protocol.md and the phone's own words** said answers came only from the Notifications page; they name the notification itself, the stray-click risk, and `toast_answers` as the off switch.
- Answerable entries now follow their notification (a toast kept once shown; a plain update, a cancel or a restarted notification server forgets it); a press that cannot be sent (a reply over 1000 characters, an unwritable state folder) says so in a notification; the README no longer states the unpressed toast as working.

