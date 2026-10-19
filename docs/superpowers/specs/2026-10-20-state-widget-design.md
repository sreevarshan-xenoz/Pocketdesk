# G1: the computer's state on the home screen (2026-10-20)

F2's plan named a home-screen widget beside the sheet. F2 shipped only the sheet. This is the widget.

## What it shows

**The widget, "Pocketdesk: how it is doing",** is for the last computer, the one the connect widget (F11) uses. It shows:
- the computer's name;
- **one line of numbers:** CPU, memory, the fullest disk, and the battery when there is one. For example, `CPU 12 % · Mem 48 % · Disk 71 % · Bat 80 %`;
- **when they were read:** "Updated 14:05" (the time today, the date before); "Not answering · from 14:05" when the last try failed; "Not answering" with nothing read; "Asking the computer…" while an ask runs.

When the computer does not share its state (`share_state` off), the widget says so in a line shorter than the sheet's. The sheet also names the remedy.

## How it gets them

**What asks, and how:**
- **A tap on the widget asks again.** It sends a broadcast to the provider itself (not exported).
- **The provider asks on a thread** (`RustCore.systemState`, the sheet's request) under `goAsync`, bounded at 8 s. It draws "Asking the computer…" at once.
- **Only the newest ask writes what it heard.** An answer that comes after the wait is drawn when it comes. An Error from the bridge is a failure, never the app's crash.

**The refresh by itself:**
- **When it runs:** at the system's widget update, every 30 minutes at most (`updatePeriodMillis`, batched by the system, never an exact alarm).
- **Its conditions:** only with Settings "State widget refreshes by itself" on (Background group, on by default), and only on an unmetered network, never over mobile data.

**Placing the widget, or a new last computer,** asks once. The app checks for one when it stops, as the connect widget does (`StateWidget.follow`).

**A computer once tried is not asked again by itself while the setting is off.** That holds whether it answered or failed.

**What is kept:** the last answer, its time and its computer's address, in the app's own preferences (not backed up). Another computer's reading is dropped once it is not the last. A reading that no longer parses counts as a failure.

`StateWidgetPlan` (JVM-tested) holds the choices:
- whether to ask, and whether a computer was tried;
- the percentages and the fullest disk;
- the time line;
- whose reading is kept.

The provider only reads and draws.

## Left out

- A widget per computer (the configure activity).
- A bigger widget with bars.

## Review

A fresh review found 0 Critical and 3 Important issues. Each was fixed in its own commit:
- **Nothing was drawn until the ask ended**, up to 8 s, so a tap seemed dead. It now draws at once, saying it asks.
- **A computer that never answered was asked at every period and every app stop.** This happened with the setting off too, because a failure kept no reading. A failure now counts as tried.
- **A reading of any age read "Updated 14:05",** and the previous computer's reading came back stale. The widget now gives the date before today, and drops another computer's reading.

Minors fixed, in two commits:
- a slow ask overwriting a newer one, and drawing nothing when it came late;
- an Error escaping the ask's thread;
- an unreadable kept reading never asked again;
- the period's ask over mobile data, now skipped;
- the spec's 9 s and its off-state words, now as built.

Owed (device, with the owner): checklist §132.
