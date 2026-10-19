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

## R6: the pass before ecosystem phase 2

Run on 2026-10-16: the A71 against this Windows box, on the LAN, with the release build of `main` after phase 1 (0461632), the phone's input off. Details are in the checklist, §84.

| | Result |
|---|---|
| Idle, no phone (570 s after a 30 s settle) | 1.35 % of one core (0.68 % on 2026-10-14), working set 34–35 MB, private 9–10 MB |
| An hour, one session from start to end (NVENC, 2560×1080) | 5.3 % of one core on average (3.4–12.6 %); private memory 82 → 87 MB, two one-minute spikes (106, 96 MB) at minutes 6 and 11; threads 52–60, handles 700–715; the phone 126–133 MB, 26 °C |
| The phone's Wi-Fi off for 30 s, 11 times | The session back 7–15 s after the Wi-Fi each time, by itself |
| The server stopped for 20 s | Back 10 s after the server listened, with no tap |

**Found and fixed:**
- **A crash with no trace.** Once, after a Wi-Fi drop, the server died with exit 139 (an access violation) as the reconnected session started, the third time this has been seen after codec negotiation (2026-10-12 twice). It did not come back in 11 more reconnects, three under gdb. A crash now leaves its backtrace in `<state>/crash.log` (PR #93), so the next one says where.
- **The control link lagged the stream** by 18 s after a server restart and by more than 50 s after a Wi-Fi drop: the session looked live while "Send to phone" failed with "phone not connected" and no push arrived. The link is now dialed at once whenever the stream comes up (PR #94); re-checked, it was back 4 s after the stream, and a file sent then arrived, MD5 matching.

**On the Linux host** (an Azure B2ats_v2 VM, no GPU, headless Sway, the release workflow's build of the same `main`; the A71 over the internet on the direct road to its public address):

| | Result |
|---|---|
| A 19-minute session (software H.264, a mostly still 720p screen, `video_quality = "low"`) | 1.33 % of one core; RSS 119 → 129 MB; the phone 130–133 MB |
| The phone's Wi-Fi off for 30 s, 3 times | Back 10–13 s after the Wi-Fi each time |
| The server restarted (20 s down) | Back within about 12 s of the server starting |
| Idle, no phone, after the session | 0.25 % of one core (Windows: 1.35 %), RSS 128–130 MB |

**A broken transfer, on a healthy link** (Windows): 108 MB of 150 arrived in 10 s, then the Wi-Fi went off for 15 s. The phone's Wi-Fi took about 60 s to rejoin this time, and its three retries (5, 15, 45 s) all met "Network is unreachable", so the download stopped at 109 MB. Sent again, it continued from there: done in 6 s, MD5 matching. The retries should wait for the network (next phase, R6i).

**More found and fixed:**
- **Two streams on a Wi-Fi return.** Both the reconnect and the network callback started one, so the computer captured and encoded two sessions for about 4 s. The callback now leaves a connect under way to finish (PR #96); re-checked: one stream.
- **The example config stopped the server** (found on the VM): `config.toml.example` and README spell `video_quality = "balanced"`, which the server refused ("unknown variant"); it now reads lowercase, and a test holds the example to the server (PR #95).
- **VAAPI was picked where it cannot run** (found on the VM): FFmpeg lists it whenever it was built with it, so Windows reported it, and a Windows PC without NVENC would have chosen it over AMF, Quick Sync and Media Foundation and ended on software. VAAPI now counts only on Linux with a render node (PR #95).

**Found, not fixed yet:**
- **Files from the computer to a phone on the internet road never arrive:** the computer dials the phone back, and the home router's NAT drops it, so a send waits its 10 minutes and fails with "the phone did not take it in time". This is the gap phase 2's transfers on the internet road close; until then the computer could say so at once.
- **Idle cost doubled on Windows** since 2026-10-14, as watchers that poll every 250 ms to 2 s were added (links, phone files and media, texts, offers, notification answers, approvals, the clipboard, the Home Assistant switch); the same build on Linux idles at 0.25 %, so the file system's cost on Windows (Defender's filter) is the likely part. Attributed and cut in R6b–R6c (below).
- **A 1 Mbit/s link.** After many Wi-Fi toggles the phone's receive rate fell to 1 Mbit/s (2.4 GHz, a strong signal); the session then reconnected every 30–60 s, since the bitrate controller's floor is 1 Mbit/s, the whole link, and a 150 MB download got 6.4 MB in six minutes before giving up. A re-join restored 150 Mbit/s.

### R6a–R6c and the crash (next phase, 2026-10-16)

**A second server on one box (R6a).** `POCKETDESK_CONFIG_DIR` names the config folder as `POCKETDESK_STATE_DIR` names the state folder (absolute only), so a throwaway server runs beside the installed one with its own ports, device id and iroh key (`server/tests/second_server.rs`, ignored; passed here). `docs/development-setup.md` says how.

**What each background loop costs at idle (R6b).** `server/src/idle_cost.rs` (an ignored test, run in release) runs each family of loops alone on a fresh runtime with no phone and reads the process's CPU time over a fixed window. On Windows it ranked the outbox (0.35 % of one core), phone media presses (0.35), the sessions mirror with its kick and control folders (0.33), links (0.23) and phone files (0.21) highest: each woke every 250 ms to 1 s to list a folder, and the mirror was rewritten every second. These are relative: each loop runs alone, the runs are separate and noisy, and each is clamped at zero, so their sum (1.67 %) over-reads the whole server (1.35 %).

**One watcher instead of the pollers (R6c).** `server/src/inbox.rs` watches the seven request folders (the `notify` crate: ReadDirectoryChangesW, inotify) and wakes the one consumer whose folder changed, which still looks every 5 s in case an event was lost; with no watcher each waits its old interval. The sessions mirror is written when what it says changes, and at least every 2 s for its readers' 5 s freshness rule (4 s at first; the review found that too tight). The loops now sum to 0.37 %; **the whole idle server is at 0.53 % of one core** (35 MB working set; 570 s after a 30 s settle, WAN and the DHT on, as R4 and R6 ran, on the reviewed build; R6 measured 1.35 % on `main` at 0461632, and R4 0.68 % on 2026-10-14).

**A crash at server stop, found and fixed (R6g's first part).** A loop that starts the server, opens a session from the A71 and stops the server 35 s in crashed about one round in 7 to 10 (exit 139, no report). A crash report written as a fault is raised (a vectored handler; one at exit never reaches the unhandled-exception filter) named it on the seventh round of the next loop: `exception 0xc0000005 … in GraphicsCapture.dll+0x105ac (executing …)`, code run inside Windows' capture library after it was unloaded. Most likely a closed capture session still finishes on the library's own threads while COM unloads it, no capture object being left; a reconnect closing the old session could meet the same race, which may be what the three crashes after a reconnected session's setup were (not shown). The library is now pinned for the process's life: 30 rounds of the same loop with no crash (at one in eight, 30 clean rounds would happen about 2 % of the time), and 10 more on the reviewed build.

### R6e–R6f: a slow link (2026-10-17)

**Reproduced in a test (R6e).** The chaos proxy can now slow its return path, where video goes, to a rate behind a drop-tail router queue (`ChaosConfig::downlink`, `SlowLink`), and lose a share of it at random (`downlink_drop_rate`). `server/tests/slow_link_session.rs` (ignored) runs the real server over loopback QUIC through it, applies the phone's 10 s stall rule, and names whatever ends the session. At 1 Mbit/s behind a 64-packet queue no timer ended it in 5 minutes, but the client saw 5–12 frames a second: QUIC's congestion control kept the queue from overflowing (89 datagrams lost in 5 minutes), so the loss controller, which acts on loss alone, held the encoder at 6.4–8 Mbit/s and the transport threw away what could not leave. With 3 % random loss on top: no end either, about 18 fps at the loss floor. So neither reproduces the A71's reconnects every 30–60 s; that run stays owed on the VM with `tc`.

**A slow link keeps a moving picture (R6f).** The video transport now counts how long it waits for its frames to be delivered and the bytes it gets through; `DeliveryCeiling` cuts the encoder's rate to 85 % of what got through when the transport waited most of two ticks (to 250 kbit/s), and climbs a quarter at a time after three ticks that used most of the ceiling with room (a still desktop holds it). Then the frame rate steps down (60, 30, 15) when the rate cannot give each frame a sixtieth of 1 Mbit/s, and back up with a quarter to spare (`slow_link_fps`; desktop Settings "Slow links", `slow_link_frame_rate`; the phone's "Full quality" keeps the session's rate). On the same 1 Mbit/s test the session settled within 100 s at 30 fps and about 480 kbit/s, with nothing more lost at the queue and no more rate changes (18–20 fps delivered, what the screen drew).

### R6g: overlapping sessions (2026-10-17)

`server/tests/overlapping_sessions.rs` (ignored; `POCKETDESK_OVERLAP_ROUNDS`, 200 by default) runs the shape that preceded R6's crashes: a session starts while the last one still runs, as a reconnect makes them, against the real capture (Windows Graphics Capture) and the real sound (WASAPI loopback), each connection with a `StreamingServer` of its own as the QUIC loop gives it. It starts as the server does (`init_process`, the crash reporter) and streams with the server's default video settings. Each round opens an old and a new session (each to its first frame; the old must still be sending once the new runs), then closes them one of three ways: the old first (a reconnect), the new first (a reconnect that failed after setup), or the new first while the old has stopped reading (a stalled old link). After each round the server must have ended both sessions within 5 s, and at the end no session may have ended with an error, no crash may be reported, and every session must have started its capture (a log records each capture, the sound's loopback and the encoders).

**Result:** a release build on this box (main at b9b690d with the fixes below): 200 rounds, 400 sessions, every one captured by Windows Graphics Capture with the sound's loopback open and a hardware encoder through FFmpeg, none ended with an error or died early, the server ended both after every round, and no crash, in 1007 s. A debug build of the first version of the test gave the same, twice. What this shows is about the test process running the server's session code, not the installed server: 0 crashes in 400 sessions bounds a crash rate at about 0.75 % per session (95 %, the rule of three), so a rarer crash would not have shown. It also does not show the test could have caught R6's crash: no control run without GraphicsCapture.dll's pin was made.

**Fixed from the review (0 Critical):**
- **The capture fallback never worked.** Each capture backend was handed the session's own token, and Windows Graphics Capture cancels its token when it fails before a first frame, so DXGI then started already cancelled (the same could befall screencopy then the portal on Linux). Each attempt now has a child token, and a capture that dies once started still ends the session.
- **The process holds COM's multithreaded apartment for its life** (`init_process`, `CoIncrementMTAUsage`). Threads enter and leave COM (capture, sound, toasts); once the last had left, COM could unload a library a WinRT factory windows-core keeps for good still points into, which is the shape of the crash inside an unloaded GraphicsCapture.dll. That library was pinned; this covers the others (the toast classes among them).
- **The test could pass silently:** it did not check a session stayed alive, that the server ended its side, or what had run; it now does (above).

**The read-through of capture and sound teardown across sessions** (the plan's second half, done by the review): each session has its own D3D device, frame pool and capture session, and its own audio client; the `FrameArrived` callback holds only a channel sender; the sound's thread runs on a child token, so none outlives its session by much. What is shared across sessions: the windows-core factory cache (kept for good; now safe with the MTA held), COM initialisation on capture and sound threads never undone (harmless with the MTA held), and capture and sound threads that finish after their session returns, overlapping the next session's start (seen by the test, which found nothing). Not looked at: whether the capture device needs `SetMultithreadProtected` within a session.

**Not covered by the test:** a toast per session as the server shows one (they would fill the owner's screen), sound actually playing (the loopback opens either way; with nothing playing no sound is encoded), and the real server's stop.

## D7: memory at rest (2026-10-19)

**Why:** Phone Link's memory is a recurring complaint; R4 and R6c measured the server's CPU at rest, and its memory only once (33 MB, R4).

**How:** `scripts/memory_at_rest.ps1` starts release builds of the server and the desktop app over a throwaway state and config folder, with no phone and the app minimized (as it rests). It lets them settle a minute, samples each every 10 s for 3 minutes, and gives the medians. Windows 11, an RTX 3060 and a Radeon iGPU, main at `0857d12`.

| Process | Working set | Private bytes |
|---|---|---|
| `pocketdesk` (the server) | 35 MB | 9.6–9.7 MB |
| `pocketdesk-app` | 88 MB | 77–85 MB |

Both held steady across the three minutes (the app's private bytes 84.9 → 84.3 MB), so nothing grows at rest.

**Where the app's memory goes:** to egui and the graphics driver, not to Pocketdesk's data. A bare eframe 0.36 window with the app's features, measured the same way, held 117 MB private on glow and 426 MB on wgpu (Direct3D 12). So the app holds less than the framework alone holds in a bare window, likely its smaller window, and the renderer already used is the lighter one. The system fonts it adds (U1) are mapped, not read in, so they count as shared pages, not private.

**Cut:** nothing. The server is small, and the app's share is the framework's and the driver's. The next step that would move the app's number is not running it at rest: the tray living in a small process of its own, the window started only when opened. That is a design change for a later cycle, not a cut here.

**Owed:** Linux (checklist §119).

**After E2 (2026-10-19):** the tray is now `pocketdesk-tray`, a program of its own, and the window closes when closed. Measured the same way (`memory_at_rest.ps1 -Tray`): `pocketdesk-tray` 13 MB working set, **2.1 MB private** (steady); the server beside it 34 MB and 9.9 MB. At rest the app's 77–85 MB is now about 2 MB.
