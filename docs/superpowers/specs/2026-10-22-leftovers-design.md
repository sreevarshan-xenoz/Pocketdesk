# N9: leftovers

Plan: `docs/research/2026-10-21-n-cycle.md`, N9.

## What

- **Typing from the tray and a key (M4 part 2).**
  - The tray's menu has "Type on the phone" for each linked phone this computer may control (both trays). The window opens its typing window for it (`app_open`'s `type`), which says what else it needs.
  - Desktop Settings "Type on the phone with a key" (`typing_hotkey`, empty is off; Windows) is registered by the tray program beside H3's clipboard key. A press opens the typing window for the linked phone in the newest session. Settings says what the tray made of it, and when it is the clipboard key's too.
- **Several links in one share.** "Open on computer" sends each link a shared text holds, once, in order, at most five (the computer opens five a minute from a phone). It stops at the first that does not go and says how many went before.
- **The phone nearly full.** One notification when a phone's free storage (L9) falls under a gigabyte, or 2 % of a large phone, again only after it has had 512 MB more. Desktop Settings "Tell me when a phone is nearly full" (`phone_storage_alerts`, on). Held while paused, and looked at only when no battery notice takes its place.

## Not in it

- The tray item and the key do not read "Control phones from their screen window"; the window says it is off.
- A phone still nearly full is told of again after the server restarts, as a low battery is.

## Review

A fresh review found 0 Critical, 2 Important and 7 Minor. Fixed:

- **Important:**
  - a nearly-full notice swallowed by a battery notice or a pause;
  - Spanish's plural without `many`.
- **Minor:** decimal sizes as the Overview's, doc comments and a test for the typing key, a key set for both, how many links went before a failure, and README's switch.
- **Left, said above:** the control setting, the restart.
