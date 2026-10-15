# Texts (SMS) on the desktop, and replies from it (roadmap N5b)

Date: 2026-10-14. Status: approved (autopilot; the user asked for it to be built now and will test it with a SIM). Follows N5a.

## Why

Reading and answering texts without picking up the phone is what people expect after notifications, as in Phone Link and KDE Connect. The test phone has no SIM, so this is built with tests and every device check is owed.

## Design

### The phone forwards incoming texts

- **Delivery.** `SmsReceiver` (manifest, `SMS_RECEIVED`, `RECEIVE_SMS`) turns each incoming text into `kdeconnect.sms.messages {messages: [{address, body, date, event: "received"}]}`. It sends it to the most recently used paired computer, from the same single worker the notifications use.
- **Off by default.** Settings "Texts on my computer" holds two switches:
  - "Show texts on my computer" asks for `RECEIVE_SMS`;
  - "Reply from my computer" asks for `SEND_SMS`.

  Contacts are not read: the desktop shows numbers, and the user can name them there later.

### The desktop keeps the conversation

- **Permission.** A new per-device permission, **Texts** ("Show its texts here and send replies"), on by default like the others, gates `kdeconnect.sms.messages` and the send result.
- **Each incoming text:**
  - is shown as a notification ("Text from +1 555…"), through the same flood guard and text cleaning as N5a;
  - is stored in `<state>/sms.json`, owner-only: the newest 1000 messages, each with its phone id, number, text, time and direction.
- **The desktop app's new Messages tab:**
  - lists conversations by phone and number, newest first;
  - shows a selected conversation's messages;
  - has a reply box. A reply is written to `<state>/sms_outbox/<id>.json` `{device, address, body}` and listed as "sending" until the phone answers.

### Replies reach the phone

The desktop cannot dial the phone. A reply waits in the outbox until that phone has a control connection; the server's outbox watcher then sends `kdeconnect.sms.request {id, address, body}` to it.

The phone keeps a control connection while "Reply from my computer" is on and it can reach the computer. `PhoneNotificationListener` is bound by Android for as long as notification access is granted, so it hosts this without a foreground service of its own. So replies need notification access too, and Settings says so. It connects once a minute when not connected, and drops back after failures.

The bridge latches `kdeconnect.sms.request` packets in a queue of their own; the listener takes them every few seconds. The phone sends each with `SmsManager` (split into parts when long) and answers `kdeconnect.pocketdesk.sms_sent {id, ok, error}`. The desktop moves the outbox entry into the conversation as sent, or marks it failed with the reason. A reply nobody takes within a day is marked failed ("the phone did not connect").

The phone refuses a request when "Reply from my computer" is off or `SEND_SMS` is not granted, and answers with why.

## Limits

- One computer receives texts: the most recently used paired one.
- A reply goes to a single number; group texts (MMS) are out of scope.
- The phone only forwards texts that arrive while sharing is on; it does not read its SMS history.

## Pieces

1. **core:**
   - `Capability::Texts`, mapping `kdeconnect.sms.messages` and `kdeconnect.pocketdesk.sms_sent`;
   - `core::local::sms`: the message store and the outbox (read, append, bounded, owner-only), shared by the server and the desktop app.
2. **server:** the sms plugin (receive, store, notify; the send result), and the outbox watcher.
3. **desktop:** the Messages tab.
4. **bridge:** `send_sms_messages`, the sms request queue (`take_sms_requests`), and `sms_sent`.
5. **app:** `SmsReceiver`, the Settings section, the reply loop in the listener, and sending with `SmsManager`.
6. **Tests:**
   - the store (bounded, order, owner-only, unreadable file);
   - the outbox (write, take, result, expiry);
   - the plugin's packet parsing;
   - the request queue;
   - JVM tests for the phone's request decision (off, no permission, ok) and parsing.
7. **Docs:** README, the checklist (all device checks owed), AGENTS.

## Settings

Everything here is the user's to set:
- **On the phone, in Settings "Texts on my computer":** "Show texts on my computer" and "Reply from my computer", both off by default, each asking for its permission when turned on.
- **On the desktop, in the app's Settings:** "Texts" (a notification for each, `sms_notify`) and "Messages history" (`sms_keep`, 1000 by default).
- **Per phone:** the Texts permission in Devices.

## As built

- **The reply path was device-verified without a SIM:**
  - the listener connected to the computer by itself;
  - the server handed over a queued reply, and the phone tried to send it;
  - the radio's answer (no SIM chosen for texts) came back as the reply's reason.

  Receiving needs a SIM, and is owed (checklist §41).
- **A reply's result is the radio's, not Android's acceptance.** Each part carries a sent callback, and the result goes to the computer once all have answered.
- **Found on the device run, fixed:**
  - **Locking the state folder.** The texts store made the whole state folder private. On Windows that stripped the access entries of every file already in it (the trust store, the device keys, `endpoint.json`), and the server could no longer read them, so the phone was refused as "not paired". `secure_fs::create_private_dir` now gives an owner entry the folder's contents inherit, and the texts store never touches the state folder's permissions. The user's folder was repaired by re-enabling inheritance; files the server keeps owner-only (the TLS key, `sms.json`) kept their entries.
  - **Duplicate hand-overs.** While the folder was unreadable, a reply's outbox file could not be removed, and the reply was handed over again on every reconnect. A reply already in the conversation is never handed over twice.
- **Fresh review (0 Critical, 7 Important), fixed in four commits:**
  - **The phone texted for any computer.** A request carries the computer whose connection brought it. The phone sends only for its own computer (the one it sends its texts to), and results go back to the computer that asked.
  - **No limits.** A reply goes only to a full number (7 to 20 digits, never a short code) and holds 1 to 2000 characters. The phone sends at most 5 a minute.
  - **Results lost.** A result was sent to whichever computer was current, or dropped during the send pause, and the per-part tracking lived in memory. It now goes to the asking computer, is never paused, and the tracking is kept in the phone's preferences.
  - **Texts lost while the computer was away.** Incoming texts wait in a queue on the phone (200 at most) and are flushed by the reply loop. The receiver no longer holds `goAsync` open.
  - **No flood guard for texts.** Text notifications share the phone notifications' per-minute limit.
  - **The history.** `sms.json` is written to a private temp file and renamed. A read that fails is an error, so a bad read never wipes the history.
  - **A reply handed to a dying connection.** A reply handed over and not confirmed is handed over again after 2 minutes while the phone is connected, at most 3 times. After that it fails with "the phone did not confirm it". The phone never sends one id twice.
- **Re-verified after the fixes (A71, no SIM):** a reply was handed over once and came back failed, "the phone has no SIM chosen for texts". The 3-hand-over limit was also seen ending a reply the phone never took.
- **Found on the re-run, fixed:**
  - **Half-open control links.** Three connections in a row were reset on the desktop's side about 20 s after connecting (the cause was not found; a fourth was not), while the phone's socket stayed open. The phone never noticed, so its reply loop never dialed again. Both ends now turn on TCP keepalive (30 s, then every 10 s), so a dead link ends within about a minute.
  - **Silent failures.** A failing reply tick was swallowed, and a panic in the bridge's tasks went to stderr, which Android discards. Both now reach logcat.

## Contact names (T8e, 2026-10-15)

- **What:** with the phone's "Names from my contacts" on (off by default; it asks for contacts access), each forwarded text carries `name`, the sender's name in the phone's contacts (`ContactsContract.PhoneLookup`). The computer keeps it (`Message.name`, one line of 64 characters) unless `sms_names` is off, and the Messages tab and text notifications read "Ann (+1 555 0100)".
- **Where:** `notify/ContactNames.kt` (`ContactNameRule`, JVM-tested), `Texts.flush`, `TextsSettings`; `core::local::sms` (`name`, `Conversation.name`, `forget_names`); the server's sms plugin; the desktop's Messages tab and Settings.
- **Fresh review (0 Critical, 3 Important), fixed:** the name was looked up on the main thread where a text arrived and stored with the queued text, so a text queued while the computer was away still went with its name after the switch was turned off (it is now looked up as the text leaves, on the worker); turning `sms_names` off kept the names already stored (they are forgotten at the next start). Minors fixed: sender IDs with letters are not looked up, and names are cut between characters.
- **Device:** no SIM: the switch and its permission were checked on the A71; receiving is owed (checklist §63).
