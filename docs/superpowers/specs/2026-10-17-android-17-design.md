# An Android 17 pass (N4)

Date: 2026-10-17. Plan: `docs/research/2026-10-17-finish-and-polish.md` §N4. Sources: Android 17's behaviour changes for all apps and for apps targeting 37 (the app targets 37), and the background audio hardening page.

## What changes, and what it means here

| Android 17 change | Here | What to do |
|---|---|---|
| Audio played, focus asked and volume set from the background are silently ignored: on 17 for every app without a visible activity or a foreground service, and for an app targeting 37 (this one) also without while-in-use capability, unless it holds the exact-alarm permission and changes only the alarm stream (still with a foreground service; the app declares none). The A71's Android 16 already logs "would be muted" | **The ring (N1)** runs from the notification listener, with no foreground service: it raises the alarm volume, and plays a tone itself when the notification cannot sound. Both are ignored. Worse, a ring that raised the volume while the app was open and put it back after the app left would leave the alarm volume at its maximum. Android grants no exemption for a notification listener, and a service started from the background gets no while-in-use capability. The ring's notification sound is played by the system, at the alarm volume the owner set | From 17, the ring does not touch the volume; the notification rings at the owner's alarm volume, and Settings says so. Sessions (screen, remote, sound) start their foreground service from the foreground, so they keep while-in-use and their sound |
| Texts holding a one-time code reach apps other than the texting app 3 hours late: WebOTP and SMS Retriever texts for every app on 17, any code text for an app targeting 37. Android says the broadcast is withheld; whether it comes after the 3 hours is not said | Texts forwarded to the computer (N5b) | Nothing the app can change: Settings says so beside "Show texts on my computer" |
| A touchpad under pointer capture reports like a captured mouse (relative motion, scroll) instead of absolute finger positions | The mouse capture (T9c) leaves touchpads out, since they sent absolute positions | From 17, a touchpad is captured like a mouse |
| IME visibility is not restored after an unhandled configuration change | The activity handles rotation and size changes itself; the session's keyboard state is not saved across a recreation, so it reopens closed either way | None |
| `IntentSender` background activity launches | No `IntentSender` use | None |
| Orientation and resizability limits ignored on large screens, with no opt-out | No orientation locks; the session already lays out for tablets and foldables (E6) | None (the emulator's tablet check) |
| `ACCESS_LOCAL_NETWORK` enforced | Asked at launch since F6 | Checked on the emulator |
| RemoteViews bitmap limit, static final reflection, read-only native libraries, implicit URI grants (18) | The widget has no bitmaps; the one reflective read (haptics) only reads; the bridge loads from the installed APK; the file-open intent grants read access | None |

## Tests

`Android17` (JVM-tested) holds the version rules: whether the ring may raise the volume, and whether a touchpad is captured.

## Check

Owed (checklist §98): the owner chose to run these later, after the API 37 emulator here stalled under both software and host rendering. On the Android 17 emulator (API 37, x86_64, against a throwaway server on this box at 10.0.2.2): the local network prompt and a session; the ring from the computer in the background with `cmd audio set-enable-hardening enable`, its notification heard (no `AudioHardening` entry for the app) and the alarm volume untouched; Settings' two notes. On the A71 (Android 16): the ring still raises the volume and puts it back.
