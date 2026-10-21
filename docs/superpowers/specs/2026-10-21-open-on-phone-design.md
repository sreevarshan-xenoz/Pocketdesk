# N3: "Open on the phone"

Plan: `docs/research/2026-10-21-n-cycle.md`, N3.

## Why

- **A notification on the computer can be answered (T10c, N3 of 2026-10-17) or cleared (L1), but not opened.** To read the whole message, see a photo or act in the app, the owner picks up the phone and finds the notification again.
- **Apple's iPhone Mirroring opens a clicked notification's app** (macOS Tahoe 26.4). Phone Link does it on Samsung phones only (Microsoft Tech Community, 2026-09-05).

## What

### The phone

- **The switch:** Settings "Let the computer open my notifications", off by default, under the notifications' settings.
- **What it offers:** with the switch on, a notification sent in full that has a tap action (`contentIntent`) carries `canOpen: true`. A notification sent without its content never does.
- **What an open does.** `kdeconnect.pocketdesk.notification_open {key, answerId}` reaches the notification listener on E1's road. `OpenRule` (JVM-tested) decides:
  - only from the phone's own computer (or a computer of mine, L8), with the switch on;
  - never while the phone is locked;
  - only a notification still showing, with a tap action;
  - at most 10 a minute, the answers' budget.
- **How it opens:**
  - **With C8's "Pocketdesk control" Accessibility service bound,** the listener sends the notification's own tap action. On Android 14+ it opts into a background start (`ActivityOptions.setPendingIntentBackgroundActivityStartMode`), which Android allows an app with a bound Accessibility service. The app opens on the phone's screen.
  - **Without the service,** the phone posts "Tap to open: <app>" carrying that same tap action, so one tap on the phone opens it.
  - **Either way the notification stays.** Android's own tap may clear it, as it would by hand.
- **The outcome** goes back as E1's `notification_answered`: "opened", "offered on the phone", or why not ("the phone is locked", "it is gone", "opening is off on the phone").

### The computer

- **The packet:** `Answer::Open` in `core::local::phone_notes`, sent as `kdeconnect.pocketdesk.notification_open`. It gets a packet of its own so KDE's are untouched.
- **What is kept:** a notification that only offers to be opened is kept now too (`PhoneNote.open`), as one that offers answers is.
- **Where the button is:** "Open on the phone" on the Notifications page; in the Windows toast; and as a Linux D-Bus action, for a notification that offers it.
- **The permission:** held to the phone's Notifications permission, as answers are. The plan named a permission of its own. Opening reaches less than answering does: it shows the phone's owner what the phone already shows, on the phone. The phone's own switch is off by default, and a further switch on the computer would add little.

## Not in it

- **"Show it here":** a cast that starts with the open, so the app shows in the phone's window. The cast's consent each time makes it a part 2.
- **Opening while locked:** Android would ask for the PIN first, on the phone.

## Safety

- **SECURITY.md limit 47:** the computer can make the phone open an app's notification, showing the app on the phone's screen. Only:
  - with the phone's switch on;
  - from its own computer;
  - while it is unlocked;
  - a notification it sent;
  - 10 a minute.
- **What the computer learns:** nothing new; it already has the notification.

## Tests

- **Core:** `Answer::Open`'s wire shape and validity; the packet's body.
- **Server:** a note that offers only Open is kept; Open is sent only for a note that offers it.
- **Desktop:** the page's and the toast's button only where offered.
- **Phone:** `OpenRule` on the JVM.
- **On the OPPO, by the runner:** a fixture notification with a tap action, opened from the computer:
  - with the service, the fixture's field shows;
  - locked, refused;
  - with the switch off, refused.
