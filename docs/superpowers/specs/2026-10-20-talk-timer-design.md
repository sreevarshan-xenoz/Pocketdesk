# H2: a talk timer in Remote (2026-10-20)

Remote is a presenter's clicker now: presenter keys (T10i), the air mouse (F4) and the laser (G4). A presenter also watches the clock, and a phone in the hand can buzz where a slide cannot. So Remote gets a talk timer.

## What it does

**Starting it:** "Timer," on a row under Remote's header, offers 5, 10, 15, 20, 30, 45 and 60 minutes. The last length chosen is marked.

**While it runs:**
- the button counts down as "12:34", then past the end as "+1:02", in red;
- a tap stops it.

**The buzzes:**
- once at five minutes left, twice at one minute, three times at the end;
- a warning as long as the talk itself is left out;
- they go as an alarm, which Do Not Disturb leaves alone;
- Settings "Talk timer buzzes" (on) turns them off.

**How it keeps time:**
- **The clock:** the uptime clock, which sleep does not stop. The time left rounds up, so the end mark comes at the end.
- **What it survives:** Remote keeps the timer's state, so a reconnect never loses it. It is saved across rotation.
- **No repeat buzzes:** the last mark buzzed is kept, so a restart never buzzes it again.
- **When it ends:** leaving Remote ends it.

The text and the marks are in `TalkTimer` (JVM-tested).

## Left out

- A spoken warning for TalkBack.
- The timer on the computer's screen.

## Review

A fresh review found 0 Critical, 3 Important and 5 Minor issues. The Important ones were fixed in two commits:
- the timer was lost whenever the link went down, since it showed only while Remote was Ready;
- the length dialog could not scroll in landscape;
- the header crowded at large text.

Minors fixed in one commit:
- marks a second early;
- a stale first frame;
- a restart buzzing a mark again;
- buzzes as touch feedback;
- a KDoc out of place.

Owed (device, with the owner): checklist §139, including the buzzes with the screen off in a pocket.
