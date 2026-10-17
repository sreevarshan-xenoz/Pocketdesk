# The desktop's Settings and words (U5)

Date: 2026-10-17. Plan: `docs/research/2026-10-17-finish-and-polish.md` §U5. From the UI audit: D-L3, D-L4, D-W2, D-W4, D-W5, X2–X4.

## The problem

- **Settings is one flat grid of about 35 rows**, everyday choices (clipboard to phones, links) beside developer ones (log level, capture backend, the DHT), and two apply rules nobody can see: most settings wait for a server restart, a few (the event log, hooks, tasks, Home Assistant) apply at once. "Files over the internet" is read live by the server but the app still asks for a restart.
- **Long labels widen the grid** past the window: a checkbox's text is its own column.
- **Empty states point nowhere:** "No phone is paired yet.", "No phone is viewing this desktop right now.", and Files' "No phone is in a session…" although linked phones and computers can take files.
- **CLI names in the UI:** "for `pocketdesk events` and scripts".
- **Three time formats:** "3 min ago" (Activity), "3 days ago" (KDE Connect), "3m 0s ago" (a phone's battery); and "5 s" ages.
- **"desktop" beside "computer":** "this desktop", "desktop sound".
- **Two quality scales:** the phone says Auto / Max / Balanced / Economy; the desktop's Settings shows the config's `Low` / `Balanced` / `High`.
- **Removal:** a paired phone is "Remove"d, a paired computer "Forget"ed, a KDE device "Unpair"ed.

## The design

1. **Sections.** Settings becomes collapsing sections, each a heading with a one-line summary of what is in it: Connection (ports, pairing required, visible on this network, files over the internet, unfinished files); Phones (video quality, H.265, slow links, several phones, hand back, clipboard to phones, links, batteries, an added screen, the mic's device on Windows, the file manager's menu, the browser); Notifications and texts; Wake; Automation (event log, hooks, tasks, Home Assistant); Other computers; KDE Connect; Advanced (screen capture, log level, your own relays, found by id). Connection and Phones start open; a section with a problem opens.
2. **Each row its own block:** the name, the control, and its note under it, wrapping at the page's width; no grid.
3. **Applies now or after a restart:** one rule (`applies_live`, tested) names the settings the running server reads again (the event log and its details, hooks, tasks, Home Assistant and its broker, files over the internet); the save bar says which of the changes apply now and which after a restart, and the restart notice after saving names them.
4. **Empty states lead somewhere:** Devices with no phone offers "Pair a phone" (the Pair page); Sessions says how to start one from the phone; Files with nothing to send to names linked phones and computers and offers "Pair a phone".
5. **One time format:** `words::ago` ("just now", "45 s ago", "3 min ago", "2 h ago", "4 days ago") everywhere a time is shown as an age, tested; durations keep `human_duration`.
6. **Words:** "computer" for the machine ("No phone is viewing this computer", "Plays this computer's sound on the phone"); no CLI names in labels ("for scripts and the events command" → "for scripts"); the video quality as the phone says it: Economy, Balanced, Max (the config's keys unchanged), with a note that a phone can lower it for its session.
7. **Unpair for pairings, Remove for saved items, each confirmed by name:** Devices' "Remove…" and Computers' "Forget" become "Unpair…", asking "Unpair <name>? It must pair again to connect." Tasks and the file manager's menu keep Remove.

## Tests

`applies_live` and the save bar's split; `words::ago` across its bands; the quality labels; the sections' membership (every setting in exactly one section, so none is lost in the move).

## Check

On this box through the app's own window, with a throwaway config and state (`POCKETDESK_CONFIG_DIR`, `POCKETDESK_STATE_DIR`): Settings' sections opened and closed, a change marked as applying now or after a restart, a narrow window wrapping, the empty states and their buttons, Unpair's question.

## As built

- **Sections** (`settings_view.rs`): `Section` (title, a one-line summary, Connection and Phones open at first) and `SETTINGS`, one table of every config key's name and section, held by a test to `Config::KNOWN_KEYS` (now public) so no setting is lost or listed twice. `show_section` draws one section's rows; the app's `settings_tab` draws each as an `egui::CollapsingHeader` whose heading is the title with the summary quieter, and puts the file manager's menu and the browser under Phones, the computers under Other computers and the KDE devices under KDE Connect (their own bold headings removed). A section with a problem (`problems_by_key`) is held open.
- **Rows**: the name, then the control and a small note, in a vertical layout that wraps at the page's width. The wake adapter's line became the note under "Wake from the phone"; "Details in events" sits under "Event log".
- **Applies now or after a restart**: `Config::changed_keys` (core, tested) lists what a save changes; `applies_live` names what the running server reads again (the event log and its details, hooks, tasks, Home Assistant and its broker, files over the internet, which the server reads at each transfer and the app had wrongly sent to a restart). The save bar says "Applies now: …" and "After the server restarts: …"; after saving, "Restart the server to apply: …" names what waits (`restart_names`, kept across Undo and cleared by a restart).
- **Time**: `words::ago` (tested) in Activity, KDE Connect, a phone's battery in Devices, Files' sent and received lists and the phone's folders.
- **Words**: "this computer's sound" in the Audio permission's help; the event log "for scripts and hooks"; video quality as Economy, Balanced and Max (`quality_label`, tested) with a note that a phone can choose lower for its session. Two strings had runs of spaces from a lost line continuation; the pairing link's note and the docs say Settings → Other computers.
- **Empty pages**: Devices offers Pair a phone; Sessions says how a phone starts one, and a stopped server's page offers Overview; Files says which phones and computers can take a file, and offers Pair a phone when none is paired.
- **Unpair**: Devices and Other computers say "Unpair…" and ask "Unpair <name>? …" with Unpair and Cancel; the notice says "Unpaired <name>".

## Review

A fresh review (0 Critical, 2 Important, 7 Minor), fixed one commit each:

- **KDE Connect's pairing requests sat in a closed section** while the server's toast sent the owner to Settings: the section is held open while a request waits, and a held section's heading says why.
- **Files promised a phone could take files "while Pocketdesk is open on it"**, which holds no link; it now names a session or "Files from the computer in the background", and Phone files names a phone with folders shared.
- The restart notice never shrank (now what is saved minus what the server read), and a failed restart cleared it; Diagnostics kept its own age format; Forget and Remove lingered in SECURITY.md, the checklist, a server toast and a comment; names such as "At most, per phone" said nothing in the save bar; the quality note now gives each step's resolution and bitrate; a stray doc line.

