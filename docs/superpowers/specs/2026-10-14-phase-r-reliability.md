# Phase R: reliability (from 2026-10-14)

Status: in progress. The plan and its order are in the roadmap (`2026-10-12-feature-roadmap.md`, "Phase R"). This file records what each step found and proved.

## R1: end-to-end on Linux, across networks

Run on 2026-10-14 against a throwaway Azure VM: Ubuntu 24.04, headless Sway, and the release workflow's Linux build. Results, and the eight bugs it found (all fixed and re-verified), are in the device checklist, §44.

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
