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
