# What would make Pocketdesk clearly worth using (research, 2026-10-15)

The next round after v0.2.0. Sources:
- **GitHub:** the most-reacted issues of RustDesk, scrcpy, Moonlight, Sunshine, Apollo, LocalSend, GSConnect, Deskreen and Weylus, with 👍 counts as of 2026-10-15.
- **Hacker News**, full threads:
  - KDE Connect, 476 points: <https://news.ycombinator.com/item?id=45557599>;
  - LocalSend, 923 points: <https://news.ycombinator.com/item?id=47933208>;
  - scrcpy v4.0, 373 points: <https://news.ycombinator.com/item?id=48114356>;
  - RustDesk unattended Wayland, 346 points: <https://news.ycombinator.com/item?id=49300759>.
- **Also:** Microsoft support pages, KDE merge requests and the tech press.

Reddit and the Play Store reviews could not be read, so demand counts come from GitHub and Hacker News. It builds on `2026-10-12-competitor-features.md` and `2026-10-14-what-users-need.md`.

Items are ranked by user value, then by effort, then by whether they can be device-checked here: a Galaxy A71 without SIM or mouse, a Windows 11 PC, and a Linux VM.

| # | Improvement | Evidence | Value / effort | Checkable here |
|---|---|---|---|---|
| 1 | **Everything over the internet path**, not just the session: clipboard, files, browsing, notifications, texts, links, battery and media over iroh, using a second ALPN on the connection the phone already dials, with transfers inside it instead of a dial-back. | "Same network only" was the top complaint in the KDE Connect thread (~14 comments, plus ~4 on VPNs and ~3 asking for a self-hosted relay) and in the LocalSend thread (~10); LocalSend #1123 has 38 👍. Today notifications and texts silently stop when the phone leaves home without Tailscale. | High / M–L | yes (block TCP 1716, or use the VM) |
| 2 | **Reply to and act on any app's notification** from the computer: actions and the reply field, the app icon, Windows toast buttons, D-Bus actions, and a reply box in the desktop app. | KDE Connect has had it since 2017 (D5991), and Phone Link has inline reply. GSConnect lost it on GNOME 45. It is the unfinished half of N5c. | High / M | yes (Element, or a debug notifier) |
| 3 | **Clipboard outside a session**: computer → phone in the background; phone → computer from a Quick Settings tile and a notification button, since Android 10+ blocks background clipboard reads. | KDE Connect MRs 142 and 145 plus a developer-mode hack; a Lemmy request for a tile; ~3 HN comments; Pushbullet put it behind a paywall. | High / S–M | yes |
| 4 | **"Send to phone"** in Explorer's SendTo and in Linux file managers (calling `pd send`), and a page from the browser. | LocalSend #115, a context menu, 49 👍; #11, a CLI, 115 👍; Windows 11's 2026 Cross-Device Resume. | Med-High / S | yes |
| 5 | **New phone screenshots and photos offered on the computer** (Open, Copy, Save), through a MediaStore observer. | Windows 11 ships it (photos into Snipping Tool), and Phone Link's Photos tab. | Med-High / S–M | yes |
| 6 | **Linux Unicode typing and capitals (F4)**: `zwp_virtual_keyboard_v1` with an uploaded keymap as wtype does (MIT), Shift on uinput (2666), the layout detected (2665). | scrcpy #712 (73 👍), #37 (34), #1055 (33). Pasted capitals arrive lowercase on Linux today. | High (Linux) / S–M | yes (Sway VM) |
| 7 | **Browse the phone's storage from the computer**, within folders granted through Android's picker (SAF). | Windows 11 put the phone in File Explorer (2024); KDE Connect's SFTP browsing (GSConnect #1610, 18 👍). | Med-High / M | yes |
| 8 | **A second monitor at the phone's shape (N11)**: VirtualDrivers' Virtual Display Driver (MIT) on Windows; Sway `create_output` or the portal on Linux. | Deskreen #42, 76 👍; Sunshine #2955, 45; Moonlight #1109, 21; Apollo and VDD (10k+ stars) exist mostly for this. | High / M (Windows), M–L (Linux) | yes (the driver install needs admin) |
| 9 | **The phone's camera as a webcam**: CameraX and H.264 up the session like the mic; Windows `MFCreateVirtualCamera` (VCamSample is MIT); Linux v4l2loopback or PipeWire. | scrcpy #241 (46 👍, since shipped) and #776 (29, open); Phone Link added it in 2024. | High / M (Linux), L (Windows) | yes |
| 10 | **The phone's screen on the computer without adb**: MediaProjection, an AccessibilityService for control, and a desktop-app window. | No open-source option without adb found; Phone Link's app mirroring is limited to a few brands; scrcpy has 152k stars. | High / L | yes |
| 11 | **Battery measured and cut; status (battery, media) carried by the background link**, not only in a session. | KDE bugs 345102 and 394818; ~4 HN comments; Phone Link's troubleshooting is mostly about background permissions. | High / M | yes |
| 12 | **Remote input without video**: trackpad, keyboard, media keys, presenter, laser pointer. | KDE Connect's remote input and presenter; bug 451360; the Unified Remote category. | Med / S | yes |
| 13 | **A Windows package identity (signed sparse MSIX)**: share target, toast buttons, Windows notifications to the phone, winget and the Store. | LocalSend #220, 43 👍; #3270, 57. | Med / M (the certificate is the hard part) | yes |
| 14 | **Unattended access on Windows** (UAC and the sign-in screen through a SYSTEM service, opt-in). | RustDesk's thread, ~8 comments; TeamViewer's UAC lockouts. | High for remote access / L–XL | partly (no locking this box) |
| 15 | **USB tethering and the phone's hotspot** as named paths, in the doctor too. | LocalSend #189 (64 👍), #850 (51), #2523 (47), #144 (36). | Med / S–M | probably |
| 16 | **Incoming-call alert, silence, and pausing the desktop's media during a call.** | KDE Discuss, bug 410149; Phone Link markets calling. | Med-High / S–M | no (no SIM) |
| 17 | All monitors at once. | RustDesk #486, 28 👍. | Low-Med / M | yes |
| 18 | A game controller connected to the phone (N8). | Apollo #957; VirtualGamePad. | Low-Med / M | no controller |
| 19 | Accessibility pass (TalkBack, AccessKit in egui). | None found; cheap hygiene. | Low-Med / S | yes |

Not recommended now: unlocking the PC with the phone (Samsung Flow dropped it after a Windows policy change, and locking cannot be tested here), and a drawing-tablet mode (no stylus).

## The order to build

1. Everything over the internet path (1): it makes "works from anywhere" true for every feature.
2. Clipboard outside a session (3).
3. Reply and act on any app's notification (2).
4. Linux Unicode typing (6).
5. "Send to phone" menus and new screenshots on the computer (4, 5).
6. Browse the phone's storage from the computer (7).
7. A second monitor at the phone's shape (8).
8. The phone's camera as a webcam (9).

Licensing: RustDesk (AGPL), Sunshine and Apollo (GPL), KDE Connect and VirtualGamePad (GPL) give ideas only. Borrowable code: scrcpy (Apache-2.0), wtype, VDD and VCamSample (MIT).
