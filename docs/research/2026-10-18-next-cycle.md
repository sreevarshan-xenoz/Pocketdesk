# The next cycle: quiet hours, conversations, and staying healthy after updates (2026-10-18)

Date: 2026-10-18, after the 2026-10-17 cycle (`docs/research/2026-10-17-next-cycle.md`). C1–C8 and C10 are done; C9 and C12 were set aside by the owner; C11 is the owner's (Android 17 on a real device).

Three inputs:
- **What users of these apps asked for, 2025–2026** (KDE Connect, GSConnect, Phone Link, LocalSend, Warpinator; sources inline). The demand signals are forum threads, bug comments and press coverage of what Microsoft is testing. Several sources are secondary, and Microsoft's are plans, not releases.
- **Platform changes:**
  - **Android 17's Advanced Protection** lets only apps marked as accessibility tools keep an Accessibility service, and revokes the others' grants (Beta 2, 2026-02-26; https://heise.de/-11210703). That is C8's control service.
  - **Android 17 hides one-time codes** from apps for three hours. The default SMS app and "approved companion apps" are exempt (https://www.techlicious.com/blog/android-17-anti-theft-privacy-features/). Whether a CompanionDeviceManager association makes Pocketdesk one is unverified.
  - **Phone Link's 2026 failures** were silent breaks after OS updates: texts gone, notification access needing a toggle (https://techcommunity.microsoft.com/discussions/windows11/windows-11-phone-link-no-longer-shows-sms-messages-after-update-to-android-17/4532121).
- **What the repo shows:**
  - The phone keeps a clipboard history (`ClipHistory`); the computer keeps none, and neither side can resend.
  - Nothing in the repo looks at Do Not Disturb, except the ring going through it.
  - The Notifications page lists notifications one by one, never as a conversation.
  - The tray has a menu but takes no files.
  - Nothing checks, after an update, that the notification listener is still bound.

The owner's standing rules hold:
- spec, then TDD; one logical change per commit;
- a fresh review per item, its findings fixed one commit each;
- device checks on the A71 against this box, and the Azure VM for Linux;
- docs with each change;
- every feature's knobs in Settings on both sides;
- no macOS or iOS;
- no power, lock or server-stop actions on the owner's computer.

## Order

| # | Item | Size | Checkable here |
|---|---|---|---|
| D1 | Health after updates: the phone checks its own listener, permissions and the control service, and says what broke on Home; Advanced Protection named | S | Yes |
| D2 | Quiet hours kept in step: the phone's Do Not Disturb holds its notifications back from the computer; "Mute the phone for an hour" from the computer | S–M | Yes |
| D3 | Clipboard history on the computer, and Resend on both sides | S–M | Yes |
| D4 | Conversations: a messaging app's notifications grouped as a thread on the computer, answered in place | M | Yes (with the throwaway chat app) |
| D5 | The tray you act from: drop files on the icon, Ring, Send clipboard, and the phone's battery | S–M | Partly (Windows; the Linux tray by the WSL stand-in) |
| D6 | Continue there: the browser tab and what is playing, both ways | M | Yes |
| D7 | What it costs the computer: the desktop app's and the server's memory at rest, measured and cut | S | Yes |
| D8 | The phone's folders as a drive on the computer (WebDAV on loopback) | M–L | Partly (mapping a drive is the owner's call) |
| D9 | Lock the computer when the phone leaves (opt-in) | S–M | By a seam only (never locks this box) |
| D10 | Calls: Answer, Decline and Silence from the computer | M | No (no SIM; the owner checks it) |

**Why this order:**
- D1 keeps what exists working: Phone Link's worst 2026 was silent breakage after updates, and C8's service is the first to lose its grant under Advanced Protection.
- D2–D4 are the requests with the most behind them that the A71 can check.
- D5–D7 are polish and cost, which the competitors are criticised for (Phone Link's memory, the Start-menu panel that is "just links").
- D8–D10 are larger or need the owner.

---

## D1. Health after updates

**Why:** Phone Link users' fix for 2026's breakages was to relink, after texts and notifications stopped silently. KDE Connect's long-standing complaints are disconnections the user never hears of.

**Done:**
- the phone checks, when it opens and after an app or system update (`ACTION_MY_PACKAGE_REPLACED`, and a changed `Build.FINGERPRINT`): whether the notification listener is bound (`requestRebind` when it is not), whether each feature's permission still holds, and whether the control service is on when its setting is;
- what broke is a card on Home, with the button that fixes it;
- Advanced Protection on (`AdvancedProtectionManager`, API 36+) is named on C8's switch: Android allows the control service only to accessibility tools.

## D2. Quiet hours kept in step

**Why:** "Don't forward notifications while the phone is in Do Not Disturb" (KDE bug 394712, commented again in 2025: https://bugs.kde.org/show_bug.cgi?id=394712); a timed mute (bug 404043); a Plasma 6.5 thread on DND conflicts (https://discuss.kde.org/t/fullscreen-do-not-disturb-ignores-per-app-show-in-dnd-and-audio-notifications-kde-connect/43359). Microsoft is testing DND in its tray flyout.

**Done:**
- the phone's "Hold back notifications in Do Not Disturb" (on by default): while the phone's interruption filter is not "all", a notification Android would not show is not sent (`NotificationListenerService.Ranking.matchesInterruptionFilter`);
- the computer's "Mute the phone for an hour" (Devices, the phone panel, the tray): the phone turns on Do Not Disturb with a schedule's end, given notification-policy access (asked from Settings), and says when it cannot;
- each phone's quiet state shown on the computer.

## D3. Clipboard history and Resend

**Why:** KDE's Remote Input request asks for a history of what was sent and a resend (2026-02-12, https://discuss.kde.org/t/kde-connect-remote-input-compose-send-history-and-send-as-clipboard-option-request/44275). Microsoft is prototyping a cross-device clipboard history.

**Done:**
- the computer keeps the last 20 text copies that crossed, either way (owner-only, off by default with a limit in Settings, never the private copies);
- the desktop app's Clipboard page lists them, with Copy and Send to a phone;
- the phone's history sheet gains Send to the computer.

## D4. Conversations

**Why:** RCS cannot be read by any app without being the default SMS app, so KDE Connect and Zorin Connect cannot show it (https://forum.zorin.com/t/zorin-connect-rcs-message-reply-support/43590). The messaging app's own notifications carry the conversation (`MessagingStyle`), and Pocketdesk already answers them (T10c).

**Done:**
- a notification with `MessagingStyle` is sent with its recent messages (sender, text, time; at most 10, under the same privacy rules);
- the computer keeps conversations by app and conversation, and the Notifications page shows them as threads with a reply box;
- a reply goes through the notification's reply action, as T10c's does.

## D5. The tray you act from

**Why:** a 2026 KDE thread asks for a device panel, dragging files onto a device and global shortcuts (https://discuss.kde.org/t/some-ideas-about-kde-connect-enhancements/49144); Windows Central called the Start-menu phone panel "just links".

**Done:**
- files dropped on the tray icon (Windows) go to the phone files reach now, as Send To does;
- the tray menu holds each phone's battery, Ring and Send clipboard, and D2's mute.

## D6. Continue there

**Why:** Windows' Cross Device Resume (Insiders, August 2025) goes phone to computer only (https://blogs.windows.com/windows-insider/2025/08/22/announcing-windows-11-insider-preview-build-26200-5761-dev-channel/).

**Done:**
- "Continue on the computer" in the phone's share sheet already opens a link (T8a); the browser extension gains "Continue on the phone" for the tab, keeping its scroll position where the page allows (a text fragment);
- what is playing: "Continue on the phone" for the computer's media opens the same item where the player is known (a YouTube or Spotify link from the media session's metadata), at its position; otherwise it says it cannot.

## D7. What it costs the computer

**Why:** Phone Link's memory use is a recurring complaint (Neowin). R6c measured the server's CPU at rest, never memory.

**Done:** the idle working set of the server and the desktop app measured (an ignored harness, as R6c's), the largest causes cut, and the numbers in the reliability spec.

## D8. The phone's folders as a drive

**Why:** Windows 11 shows the phone's storage in File Explorer through Link to Windows (preview, https://winaero.com/windows-11-will-soon-let-you-view-and-manage-android-files-wirelessly); KDE Connect mounts it over SFTP.

**Done:** the server serves the phone's shared folders (T10g) read-only as WebDAV on loopback, behind a token; the desktop app opens it and says how to map it. Mapping a drive on this box is the owner's call.

## D9. Lock the computer when the phone leaves

**Why:** the Apple Watch unlock people envy; Samsung Flow is Samsung-only, and Link to Windows added "Lock PC" in December 2025 (secondary).

**Done:** opt-in, per phone: when its control link has been gone for a set time, the computer locks. Tested only through a seam that records the lock: this box is never locked.

## D10. Calls

**Why:** the loudest long-running request (https://bugs.kde.org/show_bug.cgi?id=345686).

**Done:** a ringing call's notification on the computer gets Answer, Decline and Silence (`TelecomManager`, `ANSWER_PHONE_CALLS`), audio staying on the phone. The A71 has no SIM, so the owner checks it.

---

## Below the line

- **The phone as a webcam** (Phone Link's Connected Camera): a virtual camera on Windows 11 (`MFCreateVirtualCamera`) or `v4l2loopback`. It installs a camera device, which is the owner's call.
- **Phone apps in their own windows**: needs shell rights (wireless adb), against the no-adb promise.
- **Unlocking the computer by the phone**: a credential provider or PAM module, security-heavy.
- **Companion-app status** for one-time codes on Android 17: unverified; worth a spike when an Android 17 device is at hand.
- **Developer verification**: the owner's decision.
- **An 8-hour battery run unplugged** on several brands: the owner's (C3 and N7 left it owed).
