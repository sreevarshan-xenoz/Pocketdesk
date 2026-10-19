# D9, lock the computer when the phone leaves (design)

Date: 2026-10-19. Plan: `docs/research/2026-10-18-next-cycle.md`, D9.

## Why

Apple Watch unlock is the feature people envy. Samsung Flow does this only between Samsung devices. Link to Windows added "Lock PC" in December 2025, as a button. Pocketdesk knows when a phone's link is up, and its server can already lock the computer (the Power plugin's Lock).

## What it does

- **A per-phone switch** on the Devices page, "Lock this computer when it leaves" (`AwayLock`, off by default for every device). It is about what the computer does, as Ring is, so no packet from a phone needs it.
- **When it locks.** The server looks every 5 s. A phone with the switch on counts as present while it has a control link (a session, or a background feature that keeps one). The computer locks once all of these hold:
  - that phone was present since the server started;
  - it has been gone for the set time (Settings "Lock after the phone leaves", `away_lock_minutes`, 1 to 60, default 3);
  - on Windows, nobody has touched the computer's keyboard or mouse for that time either (`GetLastInputInfo`). A phone whose network sleeps while its owner works at the computer locks nothing.
- **Once per departure.** It locks once, and only again after the phone has come back. A phone that was never present since the server started locks nothing.
- **Several phones with the switch on:** it locks when the last of them present has left. It does not lock while any of them is present.
- **What it says:** an `away_locked` event with the phone, and the log.
- **The seam:** with `POCKETDESK_LOCK_FILE` set (tests, and checks on the owner's box), the lock is a line appended to that file instead. This box is never locked.

## Limits

- **Linux** has no idle check: the computer locks when the phone has been gone for the set time.
- **Presence is the link,** not distance. A phone that keeps its link from across the house counts as present. One whose network sleeps counts as gone, which the idle check covers on Windows only.
- **A lock, not a sign-out.** Unlocking is the computer's own.

## Testing

- **Pure, tested** (`AwayLock`): never present, then present, gone for less than the time, gone for longer, idle and not idle, locks once, back and gone again, several phones.
- **By the seam:** the server's watcher with a stand-in clock and links writes the line.
- **By hand (checklist §118):**
  - the A71 with the switch on, through the seam on this box: Wi-Fi off on the phone, a line after the set time;
  - typing meanwhile: no line;
  - the switch on the Devices page;
  - a real lock on a machine where locking is allowed (owed, not this box).
