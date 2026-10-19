# G3: the deck on the home screen (2026-10-20)

F1's deck runs on the computer from a grid on the phone, but the grid is two taps deep: the computer's menu, then Deck. A macro pad is used without looking for it, so this puts the deck on the home screen.

## What it shows

**The widget, "Pocketdesk: deck",** is for the last computer, the one the other widgets use. It shows:
- the computer's name and a ↻ that asks for its buttons again;
- **its first six deck buttons,** in two rows of three, by their labels;
- **a line saying what the last press did:** "Mute: done", "Build: waiting for the computer's owner", or the computer's reason for a refusal. A failed ask for the list says so on that line, keeping the buttons it had.

## How it gets them

**The list:**
- **What is kept:** the computer's answer (`RustCore.listDeck`, the sheet's request). It is kept per computer in the app's own preferences, and only the last computer's list is shown.
- **What fetches it:**
  - opening the deck sheet keeps its list for the widget too;
  - placing the widget, or a new last computer, asks once (`DeckWidget.follow`, when the app stops, as the state widget does);
  - ↻ asks again.
- **No period:** a deck changes when its owner edits it, so the system's period never asks.

**A press:**
- **The request:** a broadcast to the provider itself (not exported), with the button's index, its label and the computer its list was drawn for.
- **Which computer it reaches:** a press for a computer that is no longer the last is dropped, and the widget is drawn anew. The label is checked by the computer, as the sheet's press is.
- **How it runs:** under `goAsync`, the bridge is started (a tap may start the app's process), then `RustCore.pressDeck` runs on a thread.
- **What the line says meanwhile:** "Asking the computer…" is drawn at once and kept in memory only.
- **After 8 s with no answer:** the line says the computer has not answered yet, and a late answer is still drawn.
- **Repeats:** a press of a button still running is dropped.
- **The computer's rules apply, as from the sheet:** keys and text need Input, a task needs Tasks, media needs Audio, a link needs Links, and a task that asks first waits for the owner.

`DeckWidgetPlan` (JVM-tested) holds the choices:
- the six slots;
- whether to ask for the list;
- what the line says for an outcome;
- whose list is kept.

## Left out

- More than six buttons, or a widget per computer.
- A confirmation before a press. The widget is placed by the owner on their own home screen, behind the phone's lock.

## Review

A fresh review found 1 Critical and 2 Important issues. Each was fixed in its own commit:
- **Critical: the bridge was never started from the widget.** A tap that started the app's process asked with no identity or pins, so the computer saw an unknown phone. The bridge is started first now, and an Error is a failure.
- **"Asking the computer…" was saved as the line,** so an Error, a slow ask or a killed process left it for good. It is kept in memory now, and an ask past 8 s says so.
- **A button could press another computer.** After connecting elsewhere, a tap sent the old deck's index and label to the new last computer. A press now carries its computer.

Minors fixed in one commit:
- the newest-ask check and its write were apart;
- an unreadable kept list was never asked again;
- the reader cleared without the lock.

Minors left:
- the line in the language it was written in;
- a 40 by 32 dp ↻;
- "no buttons yet" when the computer has the deck switched off.

Owed (device, with the owner): checklist §134.
