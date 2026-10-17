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

- An unpackaged Windows app gets the `Activated` event only while its process runs and the toast object lives; answering from the Action Center after the server restarted does nothing (the Notifications page still answers). A COM activator, which would cover that, needs a registered class: not now.
- The reply box text is the owner's; it is written owner-only, as the page's is.

## Tests

`toast_xml` (no actions, buttons, reply, escaping, the action cap) and `toast_answer` (reply with text, an empty reply refused, a button by index, an index out of range, unknown arguments). A live `#[ignore]`d test shows an answerable toast and checks Windows accepts the XML.

## Check

On this box with the A71 and a throwaway chat app (T10c's): an answerable notification shows its buttons and reply box (read through UI Automation, never clicked). Clicking is owed to the owner.
