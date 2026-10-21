# M2: both sides' switches, part 2

Plan: `docs/research/2026-10-21-m-cycle.md`, M2. Part 1 (L7) is `docs/superpowers/specs/2026-10-21-both-sides-design.md`.

## Why

Part 1 built one side of the picture. The computer's Devices page lines up each of L7's 16 features:
- this computer's permission for the phone;
- this computer's own Settings;
- the phone's switch;
- Android's grant.

The phone sees only its own switches. Someone setting up the phone cannot tell why a feature that is on there does nothing.

A new pairing gets every default permission whoever's phone it is. The one-off guest pairing (I3) gets them too, which is far more than a guest needs: notifications, texts, input, the clipboard, and browsing this computer's files.

Requests (KDE Discuss):
- 2026-10-17: plugins whose direction is unclear, and no profile for a spouse's phone;
- 2026-07-20: settings at pairing.

## What

### M2a: the phone sees what each computer allows it

- **On the phone:** "What this computer allows" in each computer's ⋮ menu opens a sheet with the 16 features in L7's order. Each row has:
  - the feature, by the phone's own Settings name for it;
  - the phone's side: on, off, on but Android does not allow it, or "with its own computer only";
  - the computer's side: allowed, "off for this phone", "off in its Settings", or "does not say".

  A row held back where the phone wants it is shown in the error colour. The sheet asks once as it opens; Retry asks again.
- **Which computer the features reach.** The phone's background features run toward its own computer only. The notifications (and their answers) also go to the computers picked under "Other computers of mine" (L8). On any other computer, the sheet says so for each feature instead of claiming it works there. One computer saved at two addresses counts as one.
- **On the wire:** `kdeconnect.pocketdesk.allowed`. The phone sends `{}`, and the computer answers with two booleans per feature:

  ```json
  {"allowed": {"files": {"permission": true, "setting": true}, ...}}
  ```

  `permission` is this phone's per-phone permission for the feature. `setting` is whether this computer's own Settings let any phone have it.
- **Who may ask:** a phone asks only about itself, so the request needs no permission.
  - With pairing required, a device that is not paired is held back by the packet gate before it is answered.
  - Without pairing required, a device it does not know hears the defaults it runs with.
- **An older computer** never answers. The sheet then says the computer did not answer, and that one with an older Pocketdesk cannot tell.
- **One table, in core:** which permission each feature needs, and which Settings turn features off, live in `core::local::phone_switches` (`capability`, `settings_off`, `allowed_body`). The desktop's Devices page and the server's answer both read them, so the two cannot disagree.

### M2b: "Whose phone is it?" at pairing

- **On the Pair page:** above the code, "Whose phone is it?" with five choices:
  - **Mine:** the permissions as now;
  - **Someone else's:** for good, narrow (below);
  - **A guest's, for 15 minutes, an hour or a day** (I3's times): narrow, for that long.

  Changing the choice withdraws the code and mints a new one.
- **Narrow means** `DevicePermissions::someone_else`:
  - Files and Links only: the phone may send files and links to this computer, and share its own folders and photos here;
  - **not** browse this computer's files: browsing is its own permission now (below);
  - its sessions are held for the owner (`ask_first`), since a session that may not drive still shows the screen.
- **Browsing is its own permission.** `Capability::Browse` ("Browse this computer's files") gates:
  - listing a folder here (`kdeconnect.filebrowse.request`);
  - downloading a folder (`folder_fetch`);
  - a file fetched by its path (`file_fetch {path}`), checked by the share plugin, since the same packet fetches the desktop's offers, which stay under Files.

  It is on by default, so a phone paired before keeps browsing, and off for someone else's phone and a newly paired computer. Files is now "Send files" on the Devices page.
- **How the choice reaches the pairing:**
  - **The PIN file:** a `someone-else` line after the time; the lines after the time are read in any order. `pocketdesk pair --someone-else` writes it too, and `--grant` writes it for a guest. A PIN file without the line pairs as now.
  - **A PIN shown for the phone's own ask** ("Show PIN on desktop") follows a live Pair-page code that says someone else's or a guest's. The phone then pairs as someone else's, for good.
- **On the computer:**
  - **Which permissions are set** (`preset_permissions`):
    - someone else's phone, and every new guest, gets the narrow set;
    - a phone already paired, paired again as someone else's, keeps only what both allow, so nothing the owner turned off comes back on;
    - a guest grant on a phone already paired, and a computer, are left as they were.
  - **The permissions are written before the trust,** so a write that fails between them leaves only an untrusted id's entry, which the store drops as it loads.
  - **Keys:** someone else's code binds its key as a guest's does, and never takes the id of a phone that proved another key. Only the owner's own code may replace a bound key.
- **A KDE Connect device** pairs through its own flow (3b) and is not affected.
- **Afterwards:** the owner changes anything on the Devices page, as before, and the phone's mirror (M2a) shows what was given.

## Not now

- **The computer's permission names on the phone:** the phone says "the computer" and which side holds a feature back, in its own language.
- **A preset in the pairing link itself:** the computer reads its own PIN file, so the link stays as it is.
- **A someone-else phone's time on a push PIN:** a PIN shown for the phone's own ask pairs someone else's phone for good, even while the page's code is a guest's.

## Privacy

- **What the phone learns:** this computer's choices for that phone and the computer's Settings, as booleans over L7's fixed list.
- **Guests and someone else's phone:** they get far less than before. SECURITY.md limit 40 says what remains: a session the owner allows still shows the screen.
- **An older server** reading a new PIN file ignores the choice and pairs the phone as the owner's. That matters only if the app and the server are installed apart.

## Tests

- **Core:**
  - `capability` for each feature;
  - `settings_off`;
  - `allowed_body`;
  - the PIN file's preset line, with and without a grant;
  - `someone_else`, and `narrowed_to` keeping what the owner turned off;
  - Browse's gate rows.
- **Server:**
  - `allowed`'s answer: a paired phone, a stranger, no store, and pairing off;
  - `preset_permissions`: mine, someone else's new and again, a guest new and paired, a computer;
  - `someone_else_for`, for a code and for a PIN shown for the phone.
- **Desktop:**
  - `Whose` and `whose_for`, including an older guest code;
  - a code per choice;
  - a harness test that picks "Someone else's" and reads the PIN file.
- **Phone (JVM):** `AllowedRule`'s rows: each side holding back, the phone's own off, Android's refusal, an unknown answer, and another or a picked computer.

## As built

- **Core:**
  - `core/src/local/phone_switches.rs`: `ALLOWED`, `capability`, `settings_off`, `allowed_body`;
  - `core/src/local/pin.rs`: `write_preset`, and `FilePin.someone_else`;
  - `core/src/protocol/permissions.rs`: `Capability::Browse`, `DevicePermissions::{someone_else, narrowed_to}`.
- **Server:**
  - `server/src/plugins/allowed.rs`;
  - `pair.rs`: `preset_permissions` and `someone_else_for`, the guest's key rule for someone else's code, and the permissions before the trust;
  - `share.rs`: Browse for a fetch by path;
  - `pocketdesk pair --someone-else`, and the CLI's words about guests.
- **Desktop:**
  - `pair_view.rs`: `Whose`, `whose_for`, and the "Whose phone is it?" combo;
  - the Devices page's new switch;
  - `model.rs`, which reads core's table.
- **Phone:**
  - the bridge's `deck::allowed` and `nativeAllowed`;
  - `RustCore.allowed`;
  - `status/AllowedRule.kt` and `ui/AllowedSheet.kt`;
  - the menu entry and strings (English, Spanish, Tamil).

## Review

A fresh review found 1 Critical, 6 Important and 10 Minor. Each fix is its own commit, except findings 4 and 5, which share one: both live in the same lines.

- **Critical:** "files and links only" still let the phone browse this computer's home folder, and it opened screen sessions without the owner being asked.
- **Important:**
  - someone else's code could take the id of a phone that proved another key;
  - a PIN shown for the phone ignored the Pair page's choice;
  - a failed second write left the phone trusted with full defaults;
  - pairing a phone again as someone else's widened it;
  - the mirror said features worked with computers they never reach;
  - `pair --grant` narrowed its guest without saying so.
- **Minor, fixed:**
  - an older guest code shown as "Mine";
  - the answer with pairing off;
  - an older computer's words;
  - a stronger harness test;
  - reading the switches on the IO dispatcher.
- **Minor, left:**
  - `deck::allowed` sits beside `system_state` in the bridge's `deck.rs`, as F2's query does;
  - the update closure's write order is reasoned and checked by its comment, not by a test;
  - the KDE Connect note the first spec promised for the Pair page was not needed: the page already says it is for the Pocketdesk app.

## On the device (checklist §167)

- The OPPO paired to a throwaway server: the sheet's rows match the server's Devices page, with one permission and one Setting turned off; another computer's sheet; an older computer.
- Paired as "Someone else's": a file sent lands; browsing and Download this folder are refused; a session waits for Allow; a clipboard push is refused.
- The same through "Show PIN on desktop", through `pair --someone-else`, and paired again from mine.
