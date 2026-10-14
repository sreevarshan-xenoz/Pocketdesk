# The phone's notifications on the desktop (roadmap N5a)

Date: 2026-10-14. Status: approved (autopilot).

## Scope: N5 in three parts

Roadmap N5 is "notifications both ways, with actions, and the phone's notifications and SMS on the desktop". Today only the desktop → phone direction exists, with an inline reply. It is three pieces of work, done in order:
- **N5a (this spec):** the phone's notifications appear on the desktop.
- **N5b:** SMS on the desktop: new messages shown, and a reply sent from the desktop app.
- **N5c:** actions and dismissal both ways: a notification closed on one side closes on the other, and the phone's own action buttons work from the desktop.

The README already claims "receive Android notifications on your Linux desktop", but nothing on the phone sends them. N5a makes that claim true.

## What exists

- **The server shows any incoming `kdeconnect.notification`** (`plugins/notification.rs`), through `platform::notify`: a toast on Windows, `notify-send` or D-Bus on Linux. Any paired device may send one, with no limit.
- **The phone has no `NotificationListenerService`,** and no permission for one.
- **The bridge's `push_packet`** carries a packet to a desktop over the live control connection, or over a short TLS connection of its own when no session is open.

## Design

### The phone sends its notifications

`PhoneNotificationListener` is a `NotificationListenerService`. The user turns it on, and Android requires that they grant notification access in system settings. For each posted notification it decides, through a pure, JVM-tested `NotificationForwardRule`, whether to send:
- **Never:**
  - this app's own notifications;
  - ongoing ones (music, navigation, calls in progress, foreground services);
  - group summaries (their children are sent);
  - ones with neither a title nor text;
  - ones from an app the user excluded.
- **Lock-screen visibility is respected:**
  - `VISIBILITY_SECRET` is not sent at all;
  - `VISIBILITY_PRIVATE` sends the notification's public version when it has one, else only "New notification" under the app's name. This keeps one-time codes and message bodies off the desktop when the app itself hides them on the lock screen.
- **An update of the same notification** (same key, same title and text) is not sent again. Progress bars and timers update constantly.

The packet is the KDE Connect one the server already reads: `kdeconnect.notification {id, app, title, text, time, isClearable}`, with `id` the notification's key. It goes to the most recently used saved computer the phone is paired with, from one background thread. At most 20 wait; older ones are dropped, so a phone that cannot reach the desktop does not build an endless queue.

### Off until the user turns it on

Phone notifications are private. A new Settings section, "This phone's notifications", holds:
- **A switch, "Show on my computer", off by default.** Turning it on opens Android's notification access page when access is not yet granted.
- **The apps that have sent a notification,** each with a switch, so an app can be left out. Pocketdesk lists only apps it has seen notify, and keeps their package names in its own preferences, nothing more.

### The desktop decides too

- **A new permission, "Notifications", in each paired phone's Devices row.** It is on by default, like the others. Off, the desktop drops that phone's notifications. `kdeconnect.notification` moves from always-allowed to this permission; `kdeconnect.notification.request` stays allowed.
- **A flood guard:** at most 20 notifications a minute from one phone, the rest dropped with one log line a minute.
- **A cancel packet** (`isCancel`, which N5c will send) is not shown as a notification.

## Pieces

1. **core:** `Capability::Notifications`, and `capability_for` mapping `kdeconnect.notification` to it.
2. **server:**
   - the notification plugin: the flood guard, ignoring `isCancel`, and the app name in the title;
   - the pair-gate test's always-allowed list.
3. **desktop:** the Devices row gets the switch (the list is `Capability::ALL`).
4. **app:**
   - `PhoneNotificationListener` and its manifest entry (`BIND_NOTIFICATION_LISTENER_SERVICE`);
   - `NotificationForwardRule`;
   - the Settings section;
   - the sender queue, using `RustCore.sendNotification` (a new JNI export over `push_packet`).
5. **Tests:**
   - the forward rule: own app, ongoing, summary, empty, excluded, secret, private with and without a public version, repeat suppression;
   - the flood guard;
   - the capability table;
   - the desktop's Devices row.
6. **Docs:** README (fix the claim, say it is opt-in), the checklist, AGENTS.

## Device check (A71 ↔ this box)

1. Turn it on, grant access, and post a test notification. `cmd notification post` from adb is a shell notification, which counts as another app. A toast appears on Windows.
2. Exclude the shell app: the next one does not appear.
3. Turn the phone's Notifications permission off on the desktop: nothing appears.
4. Post 30 at once: about 20 appear.

## Out of scope (N5b, N5c)

- SMS.
- Dismissal sync.
- Action buttons.
- Replying to a phone notification from the desktop.
- Icons and images.

## As built

- **Device-verified on the A71 and this box,** with no session open:
  - the switch opened Android's access page;
  - a shell notification reached Windows as "New notification" (it is private);
  - of 31 within a minute, 20 were shown;
  - an app left off sent nothing;
  - the desktop's Notifications permission off was logged as denied.

  Afterwards the switch was turned off and access revoked.
- **Fresh review (1 Critical, 6 Important), fixed:**
  - **Critical, a notification echo.** The desktop-to-phone relay forwarded every notification the desktop shows, the server's own included. A phone's notification therefore came back to it and reached every other paired phone. The relay already did the same for the pairing PIN's toast. Notifications whose app is Pocketdesk are no longer relayed.
  - **Hostile text.** A phone's text is now cut to 64/200/1000 characters with control characters dropped. On Linux, markup is escaped and notify-send's arguments follow `--`.
  - **Lock-screen privacy** takes the strictest of the notification's, its channel's, the user's override and the system's "hide sensitive content". This spec first read only the notification's own.
  - **Never sent:** `FLAG_LOCAL_ONLY` notifications and other profiles'.
  - **Per-app sharing is opt-in,** not opt-out: no app's first notification leaves the phone before the user chose the app. This changes the Settings design above.
  - **One-time codes** are replaced.
  - **A failed send pauses sending for a minute.**
- **Minors fixed:** work moved off the main thread; `isClearable` comes from the notification; the permission doc comment.
- **Deferred:**
  - the label lookup for other apps on Android 11+ falls back to the package name if the system hides the app;
  - the Settings list updates on resume, not live;
  - each send opens its own TLS connection when no session is open;
  - the target is the most recently used paired computer only.
