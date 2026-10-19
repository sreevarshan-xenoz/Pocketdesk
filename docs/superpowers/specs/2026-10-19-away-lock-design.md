# D9, lock the computer when the phone leaves (design)

Date: 2026-10-19. Plan: `docs/research/2026-10-18-next-cycle.md`, D9.

## Why

Apple Watch unlock is the feature people envy. Samsung Flow does this only between Samsung devices. Link to Windows added "Lock PC" in December 2025, as a button. Pocketdesk knows when a phone's link is up, and its server can already lock the computer (the Power plugin's Lock).

## What it does

- **A per-phone switch** on the Devices page, "Lock this computer when it leaves" (`AwayLock`, off by default for every device). It is about what the computer does, as Ring is, so no packet from a phone needs it.
- **When it locks.** The server looks every 5 s. A phone with the switch on counts as present while it has a control link (a session, or a background feature that keeps one). The computer locks once all of these hold:
  - that phone was present since the server started;
  - it has been gone for the set time (Settings "Lock after the phone leaves", `away_lock_minutes`, 2 to 60, default 5);
  - nobody has touched the computer's keyboard or mouse for that time either (`GetLastInputInfo` on Windows). A phone whose network sleeps while its owner works at the computer locks nothing. Where idle time cannot be read (Linux, for now), it never locks.
- **Once per departure.** It locks once, and only again after the phone has come back. A phone that was never present since the server started locks nothing.
- **Several phones with the switch on:** it locks when the last of them present has left. It does not lock while any of them is present.
- **What it says:** an `away_locked` event with the phone, and the log.
- **The seam:** with `POCKETDESK_LOCK_FILE` set (tests, and checks on the owner's box), the lock is a line appended to that file instead. This box is never locked.

## Limits

- **Linux** never locks yet: it has no idle reading here, and a lock without one would catch the owner mid-work.
- **Reading or watching** without touching anything, while a phone lying still loses its link, can lock the computer: the default of 5 minutes is for that.
- **A gone phone** counts as gone about a minute after it went (TCP keepalive), then the set time.
- **Presence is the link,** not distance. A phone that keeps its link from across the house counts as present. One whose network sleeps counts as gone, which the idle check covers on Windows only.
- **A lock, not a sign-out.** Unlocking is the computer's own.

## Review

**Fresh review: 1 Critical, 6 Important, 7 Minor.**
- **The Critical**, fixed: the permissions test still expected every switch but Exec on by default.
- **Fixed in the watcher's rewrite:**
  - Linux (and a failed idle read) locked with no idle check; now it never locks there;
  - a phone switched off kept the lock armed;
  - an unreadable trust store or config disarmed it or shortened the wait (the last good answer stands);
  - a failed lock was never tried again, and a locker that blocks stopped the watcher (now a minute later, not waited for).
- **Said plainly:** reading or watching can lock, so the default is now 5 minutes.
- **Corrected:** the docs that claimed a watcher test.
- **Minors fixed:**
  - the seam is announced at start, and a relative one locks nothing;
  - an input stamped after the tick counts as none;
  - `lock` refuses in tests.
- **Left as they are:**
  - a guest's switch does nothing;
  - presence is the control link only.

## Testing

- **Pure, tested** (`AwayLock`): never present, then present, gone for less than the time, gone for longer, idle and not idle, locks once, back and gone again, several phones.
- **By the seam:** `lock_to` with a file writes the line; the watcher itself runs only on a server (checklist §118).
- **By hand (checklist §118):**
  - the A71 with the switch on, through the seam on this box: Wi-Fi off on the phone, a line after the set time;
  - typing meanwhile: no line;
  - the switch on the Devices page;
  - a real lock on a machine where locking is allowed (owed, not this box).
