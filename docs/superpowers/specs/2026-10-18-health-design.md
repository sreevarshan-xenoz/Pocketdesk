# D1, health after updates (design)

Date: 2026-10-18. Plan: `docs/research/2026-10-18-next-cycle.md`, D1.

## Why

In 2026, Phone Link's worst failures were silent: after an Android update, texts stopped coming and notification access needed toggling, and users found out days later (https://techcommunity.microsoft.com/discussions/windows11/windows-11-phone-link-no-longer-shows-sms-messages-after-update-to-android-17/4532121). Pocketdesk has the same exposure:
- **The notification listener.** Most background features run on it: notifications, text replies, the clipboard, files, ring, screenshots, recent photos, the phone's folders and its media.
- **Runtime permissions** behind texts, contact names, screenshots and recent photos.
- **C8's Accessibility service.** Android 17's Advanced Protection revokes it for apps that are not accessibility tools (https://heise.de/-11210703).
- **The chosen download folder (C2).**

Each is on in Pocketdesk's settings but can be taken away outside it: by Android, by an update, or by the owner in Android's settings. Today the feature just stops working.

## What it does

**One check, `Health.problems(snapshot)`** (pure, JVM-tested), returns what is on in Pocketdesk but cannot work, each problem with its fix:

| Problem | When | Fix |
|---|---|---|
| `NOTIFICATION_ACCESS` | a feature that runs on the listener is on, and access is not granted | Android's notification access page |
| `LISTENER_STOPPED` | access is granted and a listener feature is on, but the listener has not connected since the process started, even after a rebind was asked | the same page: turning it off and on restarts it |
| `TEXTS_RECEIVE` / `TEXTS_SEND` | "Show texts" / "Reply" is on without RECEIVE_SMS / SEND_SMS | the app's page |
| `CONTACTS` | "Names from my contacts" is on without READ_CONTACTS | the app's page |
| `PHOTOS` | screenshots or recent photos are on without photo access (READ_MEDIA_IMAGES, or READ_EXTERNAL_STORAGE before 33) | the app's page |
| `APP_NOTIFICATIONS` | ring, files in the background or links from the computer are on, and the app may not post notifications | the app's notification settings |
| `CONTROL_SERVICE` | "Let the computer control this phone" is on, the service is off, and Advanced Protection is not on | Android's Accessibility page |
| `CONTROL_BLOCKED` | the same, with Advanced Protection on (API 36+, `AdvancedProtectionManager.isAdvancedProtectionEnabled`) | none: the problem says Android allows it only to accessibility tools, with a button that turns the setting off |
| `SCREENSHOTS_STOPPED` | the screenshots switch turned itself off for want of photo access, and the owner has not chosen since | Settings |
| `DOWNLOAD_FOLDER` | a chosen folder whose grant is gone (`DownloadPlace.lost`) | Settings' download folder row |

Listener features are those whose setting is on: phone notifications, text replies, the clipboard in the background, answering notifications, ring, files in the background, screenshots, recent photos, the phone's media on the computer, and any shared folder.

**The listener's state.** `onListenerConnected` and `onListenerDisconnected` set a process-wide flag (`ListenerState`). When the app opens with access granted, a listener feature on, and the listener not connected, it asks `NotificationListenerService.requestRebind` once, then reads the flag again after 5 s. Only a listener still not connected then is `LISTENER_STOPPED`. Pocketdesk's activity and the listener share one process, so the flag is the truth for that process.

**Where it shows:**
- **Home:** a card "Something stopped working" lists each problem in a line, with its fix as a button. It shows while there are problems and "Tell me when something stops working" is on. "Not now" hides that set of problems until the set changes.
- **After an update:** a receiver for `MY_PACKAGE_REPLACED` (the app updated) and `BOOT_COMPLETED` (it acts only when `Build.FINGERPRINT` changed since the last boot it saw, i.e. a system update) asks the listener to rebind, waits for it (up to 8 s), and runs the check. A problem posts one notification, "Pocketdesk: something stopped working after an update", which opens Home. The receiver uses `goAsync`, so the wait stays inside the broadcast's time limit.
- **Settings:** "Tell me when something stops working" (on by default), beside "Remind me on Home" under Background.

**Not done:**
- No check of the computer's side (the Diagnostics page has its own).
- Nothing about the battery, which the Background card covers.
- Folders shared with the computer whose grant is gone are already dropped silently by `PhoneFolders.all`; that is left as is.

## Testing

- **JVM:** `HealthTest` runs every row of the table, a snapshot with nothing on (no problems), and Advanced Protection hiding `CONTROL_SERVICE`. It also covers the "Not now" set, the update rules and the build comparison.
- **Device (A71, Android 16):** each problem, made by revoking its grant with `adb shell pm revoke` / `cmd notification disallow_listener`, appears on Home, and its fix button opens the right page; a rebind brings a stopped listener back; an app update (`adb install -r`) posts the notification for what is new. A system update cannot be faked here (`BOOT_COMPLETED` is protected) and is owed.
- Advanced Protection cannot be turned on here without the owner's Google account. The code path is held by the JVM test; on the device it is owed.

## As built, and the review

- **Listener features:** forwarding, texts shown or answered, the clipboard, ring and files in the background, screenshots, recent photos and shared folders. The phone's media needs access but not the listener running (`accessFeatures`). Answering notifications needs forwarding, which covers it.
- **Posting features:** ring and files in the background. Links from the computer are on by default and post only in a session, so they are left out.
- **Home** reads the problems off the main thread (`produceState`) and draws nothing until the first read, so no card flashes. A rebind is judged after 5 s. Home records the set it found (`health_seen`), and forgets "Not now" once nothing is wrong.
- **The receiver:**
  - It waits up to 8 s for the listener, and posts only for a problem neither seen on Home nor put off (`Health.notifyAfterUpdate`).
  - It posts nothing without a saved computer. Its notification id is 39 000.
  - The build is recorded at every app start and update too, so the first system update after installing is told from a restart (`Health.systemUpdated`).
  - The listener withdraws the notice when it connects and nothing is wrong.
- **Low-RAM phones** on Android 10 or older bind no listener, so they are never called stopped. A failed Advanced Protection read is logged.
- **Fresh review: 0 Critical, 8 Important, 9 Minor.** Fixed one commit each:
  - the first update after installing went unseen;
  - every update re-announced old problems;
  - links flagged a phone that refused notifications;
  - "Not now" was never forgotten;
  - a card flashed on opening;
  - the listener was judged from one sample;
  - the feature list was wrong three ways.

  Minors fixed: low-RAM phones, a logged Advanced Protection failure, words for a permission never given, the interrupt, the id, and no notice without a computer. Left: a hint for Android's restricted settings on the card (Settings has it).
- **`requestRebind`:** the review doubted it does anything for a listener never unbound. On the A71, a listener stopped by a force-stop was live again 8 s after the app opened, and nothing was called stopped. Whether the rebind or Android brought it back is not separated.
- **Found on the device:** the screenshots switch turns itself off when photo access goes, so its problem vanished at the listener's first tick. It is now `SCREENSHOTS_STOPPED`.
- **Device-checked on the A71** (checklist §111). A real system update and Advanced Protection are owed.
