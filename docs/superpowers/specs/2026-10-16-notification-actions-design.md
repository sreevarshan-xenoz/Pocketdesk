# Reply to and act on the phone's notifications from the computer (T10c)

Item 2 of `docs/research/2026-10-15-what-makes-it-worth-using.md`, and the rest of N5c.

## The problem

The computer shows the phone's notifications (N5a) and closes them when they close on the phone (N5c part 1). It cannot answer them. A chat message read on the computer is answered by picking up the phone. KDE Connect has had replies since 2017, and Phone Link has inline reply.

## Wire (KDE Connect's names)

- **Phone → computer:** `kdeconnect.notification` gains two optional fields:
  - `actions`: the notification's plain buttons, by title, up to 3, each at most 40 characters;
  - `requestReplyId`: present when one of its actions takes typed text (a `RemoteInput`). It is the notification's own id.

  Both are sent only for a notification that went out in full. A redacted one ("New notification", a one-time code, a private one's public version) carries neither.
- **Computer → phone:**
  - `kdeconnect.notification.reply {requestReplyId, message}`, a message of at most 1000 characters;
  - `kdeconnect.notification.action {key, action}`, where `key` is the notification id and `action` a title it offered.

## Who may do what

- **The computer** sends either packet only to a phone with the Notifications permission on.
- **The phone:**
  - acts only on packets from its own computer, the one its notifications go to (as texts replies do);
  - acts only on a notification it still shows, and only with a button it offered;
  - acts only when Settings "Let the computer answer notifications" is on. It is off by default, since an answer goes out as you.
  - acts at most 10 times a minute.

## Desktop

- The server keeps the phone notifications that can be answered in `<state>/phone_notifications.json`:
  - owner-only;
  - the last 30;
  - a notification is dropped when the phone closes it;
  - the file is emptied at start.
- The desktop app's Messages tab gains "Phone notifications": each one shows its app, title and text, its buttons, and a reply box when it takes one.
- A press or a reply is a request file in `<state>/notification_out/`, which the server sends, as the link and texts outboxes do. A request lasts 60 s.

## Phone

- The notification listener remembers, for each notification it sent in full, its `Notification.Action`s by title and the one with a `RemoteInput`. It keeps at most 100, and forgets each when it is removed.
- The bridge keeps the computer's requests, with the computer they came from; there are at most 20.
- The listener's 5 s tick runs them:
  - a reply fills the `RemoteInput` and sends its `PendingIntent`;
  - an action sends its `PendingIntent`.

  Each result is logged; no answer goes back in this step.

## Tests

- **Core:**
  - the store: last 30, drop on cancel, owner-only writes;
  - the outbox: request, take, TTL.
- **Server:**
  - only full notifications with actions are kept;
  - the watcher sends only to a permitted, connected phone.
- **Bridge:** the latch's bound, fields, and source.
- **Kotlin (JVM):** `NotificationActionRule` (which actions to offer and their titles; the gates: setting, own computer, known notification, known action, budget).
- **Device:** a notification with a reply action, from a probe app in the test APK, as the T8d probe did. Then reply from the desktop app's Messages tab, and check the probe received the text.

## As built

- **The answers have their own tab**, "Notifications", next to Messages, rather than sharing the texts' conversation view.
- **The probe** was a throwaway app built as an uncommitted Gradle module. A test APK cannot post as another app.
- **Found on the device run, fixed:**
  - **Nearly every notification reached the computer as "New notification" (N5a).** Every Android notification is private unless its app says otherwise, and the lock screen hides a private one's content only when the user hides sensitive content. The rule hid it always. Now an app's own private follows the user's choice; a channel's or the user's private for an app still hides it.
  - **Answers were dropped at once while the phone had no live link,** because its notifications went over one-off connections. The phone now keeps a link while answering is on, and the server holds an answer for its minute.
- **Device-checked (A71 → Windows), checklist §66:** a reply through `RemoteInput`, a button, and an unoffered button refused.
