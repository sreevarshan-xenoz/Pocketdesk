# A phone notification closed on the phone closes on the desktop (roadmap N5c, part 1)

Date: 2026-10-14. Status: approved (autopilot). Follows N5a.

## Why

With N5a, the phone's notifications appear on the desktop. When the user reads or clears one on the phone, its copy stays on the desktop: in the Windows Action Center, or in a Linux notification server's history. Clearing a chat on the phone should clear it everywhere.

## Scope

N5c in the roadmap is "actions and dismissal both ways". This part is the half that can be built and verified here:
- **In:** a notification removed on the phone (read, swiped, cleared by its app) is removed from the desktop.
- **Later:**
  - The desktop → phone direction (closing a mirrored desktop notification on the phone). On Windows it needs the notification listener, which needs a package identity this box's server does not have.
  - Action buttons (Reply, Mark as read) carried across. They need Windows toast activation, a COM activator or a protocol handler, for an unpackaged exe.

## Design

### The phone

`PhoneNotificationListener.onNotificationRemoved` sends `kdeconnect.notification {id, isCancel: true}` for a key it sent before. It remembers the keys it sent, in the rule's bounded memory. A key it never sent is not mentioned, so the desktop learns nothing about notifications it never saw.

### The desktop

The server knows each phone notification by `(phone id, notification id)`:
- **Windows:** each toast is shown with a tag (the first 16 hex digits of the SHA-256 of phone id and notification id) and the group "phone". A cancel removes it from the notification history (`History().RemoveGroupedTagWithId`), which takes it off screen too. Tags never repeat across phones, so one phone cannot clear another's. The removal is scoped to the phone that sent the cancel.
- **Linux:**
  - A phone notification is shown over D-Bus `Notify`, not notify-send, so the server keeps the id it returns: in a bounded map, the newest 500.
  - A cancel calls `CloseNotification` with that id.
  - When D-Bus fails, notify-send still shows the notification, without the means to close it.

A cancel goes through the same Notifications permission as the notification, and does not count against the flood guard. A cancel for an unknown id does nothing.

## Pieces

1. **server:**
   - `platform::notify_tagged(summary, body, tag)` and `platform::remove_tagged(tag)`;
   - the plugin's cancel path and the tag;
   - the Linux id map.
2. **app:** the listener sends the cancel for keys it sent.
3. **Tests:**
   - the tag (stable, phone-scoped, 16 hex);
   - the cancel decision (a sent key is cancelled, an unknown one is not);
   - the Linux id map's bound;
   - a live ignored Windows test: show a tagged toast, remove it, and it is gone from the history.
4. **Docs:** README, the checklist, AGENTS.

## Device check (A71 ↔ this box)

Post a shell notification (shared), see the toast in the Action Center, cancel it on the phone (`cmd notification cancel`), and it leaves the Action Center.

## As built

- **Device-verified on the A71 and this box:**
  - a shared shell notification's toast was in the Action Center with its tag;
  - swiping the notification away on the phone sent one cancel, and the toast left the Action Center;
  - a live ignored test shows a tagged toast and takes it back.
- **This Android has no `cmd notification cancel`,** so the device check dismisses from the shade, as a person would.
- **Owed (checklist §40):** the Linux host, and a chat app that clears its notification when read.
- **Fresh review (0 Critical, 1 Important), fixed:**
  - **Linux showed an updated notification as a second one,** so a cancel closed only the newest. An update now replaces the id shown before for its tag (`replaces_id`).
  - **Minors fixed:**
    - a notification without an id is shown untagged, so it never replaces another;
    - the D-Bus app name is "Pocketdesk" again, matching notify-send;
    - the Linux id bookkeeping is tested on every OS.
  - **Accepted:**
    - with no session open, a cancel can race its own show on a separate connection, and the notification then stays until it expires;
    - with the Notifications permission turned off, cancels are dropped too.
