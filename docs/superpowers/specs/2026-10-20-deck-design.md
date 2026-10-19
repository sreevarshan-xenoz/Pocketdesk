# F1: the deck (2026-10-20)

Plan: `docs/research/2026-10-20-next-cycle.md`.

## What

The phone as a macro pad, the way Macro Deck, Deckboard and Touch Portal work, but on the link the phone already keeps with its computer:
- the owner sets up buttons in the desktop app's Settings;
- the phone shows them as a grid;
- a press does the button's action on the computer, without a session.

## The buttons

`[[deck]]` in config.toml, at most 48:

```toml
[[deck]]
label = "Mute mic"
keys = "ctrl+shift+m"

[[deck]]
label = "Build"
task = "build"

[[deck]]
label = "Play/Pause"
media = "playPause"     # or "next", "previous"

[[deck]]
label = "Calendar"
link = "https://calendar.example.org"

[[deck]]
label = "Sign-off"
text = "Thanks, talk soon!"
```

- **Exactly one action per button:** `keys`, `task`, `media`, `link` or `text`.
- **The label:** 1–40 characters, no control characters.
- **`keys`:**
  - up to 5 key names joined by `+`, without case;
  - each a name in the evdev table (`a`, `f5`, `enter`, `pageup`, …) or an alias:
    - `ctrl`, `shift`, `alt`, `win`/`super`/`meta`/`cmd`;
    - `esc`, `del`, `pgup`/`pgdn`, `return`, `space`, `up`/`down`/`left`/`right`.
- **`text`:** at most 1000 characters.
- **`link`:** http(s), at most 2048 bytes; held by the computer to the same rule as a phone's link (`link_problem`).

`core::local::deck` parses and checks them (`DeckButton::action`). A button that does not check is shown in Settings with its problem and left out of the phone's list.

## The wire

- **`kdeconnect.pocketdesk.deck`, phone → computer:** answered with `{buttons: [{index, label, kind}]}`, the buttons that check, `kind` one of `keys`, `task`, `media`, `link`, `text`. Never the keys, the link or the text themselves. A computer with no buttons answers an empty list.
- **`kdeconnect.pocketdesk.deck_press {index, label}`, phone → computer:** the button at `index`, if its label is still `label` (so a press never runs a button changed since the phone listed). Answered with `kdeconnect.pocketdesk.deck_result {index, ok, reason}`.
- **Permissions:**
  - neither packet has a capability of its own;
  - the action is held to the phone's permission for it:
    - keys and text: Input, and the phone may drive (N7's `may_drive`);
    - a task: Tasks (and its ask first);
    - media: Audio, as the Sound sheet's media buttons are;
    - a link: Links.

  A refusal answers with why, e.g. "Input is turned off for this phone on the computer".
- **Limits:** 30 presses a minute per phone; more are refused ("too many presses").
- **Desktop Settings "Deck":** on by default. Off, the list is empty and a press refused ("the deck is turned off on the computer").

## The computer

- **`server/src/plugins/deck.rs`** does the action:
  - keys: its own lazily made `InputInjector`. Modifiers are pressed in order and the last key tapped, then all are released in reverse. Each code goes through `InputPacket::KeyEvent`, so every backend takes it.
  - text: `InputPacket::Text`;
  - a task: `tasks::start_for_device` (Home Assistant's start, given the phone's id and its asker);
  - media: `media::command` on a blocking thread;
  - a link: `links::open_in_browser` after `link_problem`.
- **Events:** `deck_pressed` with the kind and the label, never the keys' or the text's content.
- **Settings "Deck" (Automation):** the rows for buttons, as Tasks has:
  - a label;
  - a kind;
  - its value;
  - Remove and Add, with each button's problem said under it.

## The phone

- **Home:** "Deck" in each computer's ⋮ menu opens a sheet. A grid of its buttons, 3 a row (Settings "Deck buttons per row": 2, 3 or 4), each labelled with its kind's icon.
- **A press:** shows "Done" or the reason on the button for 2 s, with haptics.
- **The session menu** opens the same sheet.

## Tests

- **Core:** parsing and checking (each kind, keys and their aliases, the limits, one action only).
- **Server:**
  - the listing leaves out bad buttons;
  - a press checks the label, the permission, the deck switch and the rate;
  - keys become press-and-release in order (the injector behind a seam).
- **Bridge:** the listing and press calls.
- **JVM:** the grid's layout rule and the result's words.
- **Owed (device, with the owner):** a button of each kind from the A71; keys into a scratch window (input on this box); a task asking first.

## As built

As specified. Details the spec did not fix:
- **Keys:** pressed through one injector for the server (`deck::INJECTOR`), used on blocking threads only and made at start when the deck has buttons (`deck::warm_up`).
- **Links:** opened or shown as a phone's own link, following "Links from phones".
- **Tasks:**
  - a task button starts its task as Home Assistant's do;
  - the owner's prompt says "a phone's deck";
  - one that asks first is answered `waiting`, which the button shows as "Waiting for the computer's owner".
- **The phone:** presses go one at a time.
- **Not checked on a device** (the owner's night of features, 2026-10-20): checklist §126.

## Review

A fresh review found 0 Critical and 5 Important issues, fixed one commit each:
- a press that failed midway left a modifier held (now every key down is let go, past failed releases);
- the first combination on Linux could lose its first key (the injector is now made at start);
- typing ran on the phone's control loop (now on blocking threads);
- two presses in flight could take each other's answers (now one at a time);
- a hand-edited 49th button could be pressed though never listed.

Minors fixed: a task that asks first said "Done", and a deck link ignored "Links from phones".

Minors left:
- a refused press goes to the log, not the Activity page;
- a task button naming no task shows no problem in Settings (the phone says "unknown");
- with pairing off, an unpaired device may list the labels (its presses are refused).
