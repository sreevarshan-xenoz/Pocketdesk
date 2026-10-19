# G2: take a photo from the tray and the Overview (2026-10-20)

F3 put "Take a photo with the phone" on the Phone page only. This puts the same ask beside each phone's other actions.

## What it does

**Where it is offered:**
- **The tray's menu:** "Take a photo" is under each paired phone, after Send file… (`TrayAction::TakePhoto`). The window's own tray and `pocketdesk-tray` both handle it.
- **The Overview:** a "Take a photo" button is on each phone's row.

**When it is enabled:** when files reach the phone (`row.file`: the Files permission, and the phone in a session or linked with files), as Send file… is. That is the rule the server's take request needs: the Files permission and a control link.

**What a press does:** it writes F3's `Ask::Take` request (`phone_photos::request`). The server sends it, the phone offers its camera in a notification, and the photo lands with the received files.

**What the computer cannot see:** whether the phone's "Photos the computer asks for" is on. The Overview's hover text and its notice say the phone needs it.

## Review

A fresh review found 0 Critical and 0 Important issues.

It found 3 Minor ones. Two are fixed in one commit: the switch the phone needs is now named, and README's F3 line names the tray and the Overview.

Left as it is:
- `pocketdesk-tray` says nothing after a press. Ring, Mute and Send the clipboard behave the same.
- Repeated presses queue repeated asks. The phone shows one such notification at a time, so a repeat replaces it.

Owed (by hand, with the owner): checklist §133.
