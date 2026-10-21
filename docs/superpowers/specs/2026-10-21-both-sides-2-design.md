# M2: both sides' switches, part 2

Plan: `docs/research/2026-10-21-m-cycle.md`, M2. Part 1 (L7) is `docs/superpowers/specs/2026-10-21-both-sides-design.md`.

## Why

Part 1 built one side of the picture. The computer's Devices page lines up each of L7's 16 features:
- this computer's permission for the phone;
- this computer's own Settings;
- the phone's switch;
- Android's grant.

The phone sees only its own switches. Someone setting up the phone cannot tell why a feature that is on there does nothing.

A new pairing gets every default permission whoever's phone it is. The one-off guest pairing (I3) gets them too, which is far more than a guest needs: notifications, texts, input and the clipboard.

Requests (KDE Discuss):
- 2026-10-17: plugins whose direction is unclear, and no profile for a spouse's phone;
- 2026-07-20: settings at pairing.

## What

### M2a: the phone sees what each computer allows it

- **On the phone:** "What this computer allows" in each computer's ⋮ menu opens a sheet with the 16 features in L7's order. Each row has:
  - the phone's own switch name, as its Settings says it;
  - the phone's side: on, off, or on but Android does not allow it;
  - the computer's side: allowed, "Off for this phone on the computer", or "Off in the computer's Settings".

  A feature on here but held back there says which side to change. The sheet asks once as it opens; Retry asks again.
- **On the wire:** `kdeconnect.pocketdesk.allowed`. The phone sends `{}`, and the computer answers with two booleans per feature:

  ```json
  {"allowed": {"files": {"permission": true, "setting": true}, ...}}
  ```

  `permission` is this phone's per-phone permission for the feature. `setting` is whether this computer's own Settings let any phone have it.
- **Who may ask:** a phone asks only about itself, so the request needs no permission. A device that is not paired gets `{"allowed": {}}`.
- **One table, in core:** which permission each feature needs, and which Settings turn features off, live in `core::local::phone_switches` (`capability`, `settings_off`). The desktop's Devices page and the server's answer both read them, so the two cannot disagree.

### M2b: "Whose phone is it?" at pairing

- **On the Pair page:** above the code, "Whose phone is it?" with three choices:
  - **Mine:** the permissions as now;
  - **Someone else's:** files and links only. No input, clipboard, notifications, texts, sound, mic, calls, commands, ringing, muting or showing its screen;
  - **A guest:** someone else's permissions, for 15 minutes, an hour or a day (I3's choices).

  Changing the choice withdraws the code and mints a new one, as the guest choice does now.
- **How it reaches the pairing:** the choice is kept with the PIN, on a fourth line of the PIN file (`someone-else`). A PIN file without the line pairs as now.
- **On the computer:** the pairing handler sets those permissions as it trusts the phone (`DevicePermissions::someone_else`). A guest gets them too. Each can be changed later on the Devices page as before.
- **A KDE Connect device** pairs through its own flow (3b) and is not affected. The Pair page says the choice is for Pocketdesk phones.
- **Afterwards:** the phone's mirror (M2a) shows what was given.

## Not now

- **The computer's permission names on the phone:** the phone says "the computer" and which side holds a feature back, in its own language, rather than the computer's English label.
- **A preset in the pairing link itself:** the computer reads its own PIN file, so the link stays as it is.
- **Presets for computers:** a computer pairs with its own narrow set (2f).

## Privacy

- **What the phone learns:** this computer's choices for that phone and the computer's Settings, as booleans over L7's fixed list. It does not learn how the computer is set up beyond whether each feature reaches it.
- **Guests:** they get less than before (SECURITY.md limit 40).

## Tests

- **Core:**
  - `capability` for each feature;
  - `settings_off` for each setting;
  - the answer's body for a phone's permissions and a config.
- **Server:** the plugin answers a paired phone and gives an unpaired one nothing. The PIN file's preset line is read and written.
- **Pairing:** "someone else's" gives its permissions, a guest too, and "mine" the defaults.
- **Desktop:** the Pair page's choice, through the harness.
- **Phone (JVM):** the mirror's rows, covering each side holding a feature back, the phone's own off, and Android's refusal.

## On the device (checklist §167)

- The OPPO paired to a throwaway server: the sheet's rows match the server's Devices page, with one permission and one Setting turned off.
- Paired as "Someone else's": the sheet shows files and links only, and a clipboard push is refused.
