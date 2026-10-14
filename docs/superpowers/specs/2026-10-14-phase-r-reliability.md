# Phase R: reliability (from 2026-10-14)

Status: in progress. The plan and its order are in the roadmap (`2026-10-12-feature-roadmap.md`, "Phase R"). This file records what each step found and proved.

## R1: end-to-end on Linux, across networks

Run on 2026-10-14 against a throwaway Azure VM: Ubuntu 24.04, headless Sway, and the release workflow's Linux build. Results, and the eight bugs it found (all fixed and re-verified), are in the device checklist, §44.

## R2: long sessions and recovery

Run on 2026-10-14: the A71 against this Windows box, on the LAN, with the release build and the phone's input off (view-only). Details and the sampler are in the checklist, §46.

**A 35-minute session, then a 30-minute one after the fixes:**

| | First run (35 min) | After the fixes (30 min) |
|---|---|---|
| Server CPU while streaming 2560×1080 (NVENC) | 10–24 % of one core | 10–13 % |
| Server private memory | 84 → 96 MB | 85 → 89 MB |
| Server handles | 674 → 705 | 677–683, flat |
| Phone app memory (PSS) | 142–149 MB, flat | 159–163 MB, flat |
| Phone temperature | 26 °C, flat | 26 °C, flat |

The phone was on USB power, so these runs say nothing about battery (R4).

**Found and fixed:**
- **The control connection did not come back after a network drop.** The stream recovered by itself 12 s after the Wi-Fi returned, but only the clipboard loop dialed the control connection, and only when a send failed. Its poll silently fell back to throwaway connections (8 a minute), and the desktop's pushes (file offers, the HUD, wake details) stopped for the rest of the session. A session now keeps its control connection (`ControlLinkEffect`, backing off to a minute and resting in the background). Re-run: one reconnect, then steady.
- **A desktop server restart left the phone on "The video link dropped" until someone tapped Retry.** A link that went away now reconnects by itself, backing off from 2 s to 30 s, for ten minutes (`AutoReconnect`, with a Settings switch). An end the desktop chose waits for the person. Re-run: back 4 s after the server listened again, with no tap.
- **Two control connections at session start** (fresh review): the keeper and the clipboard loop could both dial. The bridge now dials one at a time and closes a replaced connection.

**Fresh review (0 Critical, 3 Important, 7 Minor), fixed:**
- the backoff started over at every QUIC connect, so a stream that connected and then failed retried forever (now only after a picture played for 10 s);
- with no cached internet identity, a timeout was final;
- the double dial above;
- plus transport errors read as the desktop's choice, "pin" matching "ping", Retry not restarting the window, overlapping rebuilds, the keeper's backoff and background, and no word when retrying stops.

**The hour-long run found a leak, fixed:**
- For its first half hour, with one session, the server held steady (private memory 82 → 87 MB).
- Then the phone's watchdog gave up on 10 s of silent video and reconnected. That is right, but the desktop kept the old session, still encoding at full rate, so memory reached 177 MB with 83 threads, and CPU doubled.
- Stopping a stream only cancelled the phone's tasks and dropped its handle; a clone of the connection outlived it, and keep-alives held the link open.
- Fixed both ways:
  - the phone now closes the connection on the wire when it stops a stream;
  - the desktop ends a session that has heard nothing from its client for 30 s, since every client reports its link once a second.
- Re-verified: a monitor switch closes the old session within 0.1 s and leaves one live session.

**Not done in R2:**
- **Why the video went silent for 10 s** in the middle of a healthy session is not known. The loss controller had been backing off just before, so the link was congested. It is recovered from now, not explained.
- **An hour-long run with one session from start to end,** to confirm the memory levels off.
- **The computer sleeping** is the user's to run (AGENTS.md: no power actions on this box).
- **A Linux host** for any of this.
- **Resuming a file transfer** across a drop: a transfer that breaks fails and is sent again whole.

## R3: adversarial security

An audit listed, for each attack, whether a test existed, whether the code failed closed, and whether it said why. These were fixed, each with a test:

| Case | Before | Now | Test |
|---|---|---|---|
| A phone unpaired mid-session (CLI, app, guest expiry, edited store) | kept streaming until it left | ends within a second | `a_session_whose_phone_is_no_longer_paired_is_ended`; checked on the VM |
| An unpaired phone with an id like `all`, or a prefix of another phone's id | (the new sweep) would have ended other phones' sessions | each session ended by its own id | `ending_a_session_by_id_touches_no_other` |
| Input, Sound or Microphone turned off mid-session | only the desktop app ended it, if it had seen the session | the server ends it within a second | `a_session_whose_permissions_were_narrowed_is_ended` |
| One endless control line, before pairing | memory grew without limit | 4 MiB, then the connection ends | `a_packet_line_has_a_limit_too` |
| A v2 control frame | 16 MB | 4 MiB | `test_oversized_payload` |
| Many connections from one host | no limit | 8 per address, 128 in all; the WAN listener 128 in all | `one_address_gets_only_its_share`, `the_server_as_a_whole_has_a_limit` |
| Another phone ends or re-arms privacy mode | allowed | refused, and logged | `only_the_phone_that_grabbed_may_refresh_or_release` |
| A device id with control characters on the stream | reached logs and the approval prompt | refused (no id: the gate refuses) | `a_device_id_is_short_and_printable` |
| A PIN file without its minting time | a PIN that never expired | no PIN | `the_grant_line_survives_and_a_pin_without_a_stamp_is_no_pin` |
| A PIN that paired, whose file could not be deleted | reusable | spent for the run (by PIN and minting time, in one step) | `a_pin_that_paired_stays_spent` |
| One phone taking every transfer slot | possible | two per phone, four in all | `one_phone_cannot_take_every_transfer_slot`, `a_phones_next_file_is_not_refused_while_its_last_one_finishes` |
| Garbage on the input stream | per-case tests only | 20 000 random inputs, no panic | `decoding_garbage_never_panics` |
| Refusals at the default log level | debug only, or not logged (v2 streams) | the first per connection at info; v2 stream errors as warnings | — |
| A v2 handshake's id or name with control characters, or megabytes long | logged and registered raw | refused | `a_v2_peer_needs_a_short_printable_id_and_name` |
| A device id with a bidi override or a zero-width character | passed | refused | `a_device_id_is_short_and_printable` |
| A forged QUIC source address filling a phone's share | possible (after the budget) | an address counts once it answers a Retry | `a_quic_attempt_is_counted_only_once_its_address_is_proven` |
| One failed read of the trust store | (the new sweep) would end every session | judged after three unreadable ticks | `one_unreadable_store_does_not_end_every_session_but_a_lasting_one_does` |
| A folder listing over the packet cap | ended the phone's connection | listed in part, and the phone says so | `a_huge_folder_is_listed_in_part_within_one_packet` |
| An unpaired phone's packet, end to end | gates tested only as pure functions | through the real handler and TLS, against a throwaway state folder | `an_unpaired_phones_packets_are_never_dispatched` |

**Fresh review (0 Critical, 5 Important), all fixed:** the stale capability docs (a failing test), forged QUIC addresses against the new budget, a multi-file share losing its next file to the per-phone limit, the v2 handshake's unchecked id and name, and the WAN listener outside the budget. Minors fixed: spent PINs keyed by minting time and checked in one step, invisible characters in ids, the denied-packet log throttled per type, a refused privacy request no longer locking the screen, `POCKETDESK_STATE_DIR` required to be absolute, one unreadable store, the folder listing, and stale docs. The rest are in SECURITY.md's limits 5 and 6.

**A seam for these tests.** `POCKETDESK_STATE_DIR` and `core::local::paths::use_state_dir` let tests point the server's gates at a throwaway state folder, never the user's.

**Checked and fine:**
- offers and approval answers are taken once;
- the kick and control request files are taken once, by UUID name;
- browse and download paths;
- received file names;
- clipboard images (by hash, never a path);
- the mic relay, which starts nothing before a session the gate allowed.

**Known and accepted (SECURITY.md lists the real limits):**
- **Keyless phones over the WAN.** A phone paired before device keys, with no key bound yet, is admitted over the WAN by its id alone.
- **The PIN guess counter is global.** A hostile device can spend the owner's PIN with wrong guesses; it cannot pair.
- **Some refusal paths still have only unit tests:** v2 with a wrong key, and a wrong key on the control channel, end to end.

## R4: idle cost

**The desktop, measured on 2026-10-14** (Windows, the release build, no phone connected, WAN and the DHT on): over 120 s after a 60 s settle, the server used 0.68 % of one core (0.04 % of the 16 logical CPUs). Its working set was 33 MB (private 8.7 MB), with 26 threads and 363 handles. The script is in the checklist, §45.

**Still owed:** the phone's battery over an hour with the app in the background, the network traffic while idle, and the Linux host.

## R5: honest docs

Done on 2026-10-14 (branch `docs/r5-honest-docs`):
- **Port forwarding.** README's opening line said "no port forwarding required" without the exception, and named Tailscale as how it connects. The new "Before you start" section has a table of each way a phone reaches the computer. A router forward is needed only to wake a sleeping computer from outside, and on your own relay's machine.
- **First contact and the internet path, up front.** README and the top of SECURITY.md say: scan the QR code (a typed address is trust on first use), and relays and the DHT see who talks, and the home IP.
- **What works where.** A Linux / Windows table, with where each host was device-checked, and the phone's minimum Android version.
- **A release checklist,** `docs/release-checklist.md`, linked from CONTRIBUTING: CI, versions, the capability docs on each OS, a dry run with a release-signed APK, the archives on real machines, and what to do if a release is wrong.
- **Found on the way:** the docs asked for Rust 1.80, but the tree needs 1.95 (egui 0.36; iroh needs 1.91), now declared as `rust-version` in Cargo; and Tailscale was listed as needed for testing.
- **Fresh review (5 Important, 12 Minor), all fixed:** the internet row hid that the phone must meet the computer at home first and that only the session crosses iroh (clipboard, files, notifications and texts need the same network or Tailscale); what the DHT and relays see of the phone; elevation never reaching the UAC prompt; the device-checked row overstating Linux; and the Rust minimum, really 1.95 (now `rust-version` in Cargo). The internet path stays always on, documented as such; no switch was added.
