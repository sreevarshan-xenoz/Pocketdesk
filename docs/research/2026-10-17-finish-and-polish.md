# The next phase: finish and polish, then what people ask for (2026-10-17)

Date: 2026-10-17, after v0.4.0 (phases 1–3 of the ecosystem plan, R6). Two inputs:
- **A UI/UX audit of both apps** (the phone on the A71: Home, every Settings page, the add form; the desktop app from its code). 38 issues and 5 larger improvements. Two block basic use: most desktop tabs cannot scroll (Settings' Save is off-screen at the default size), and the phone's Back closes the app.
- **Research into what this kind of app's users ask for in 2025–2026** (KDE Connect, GSConnect, Phone Link, LocalSend, scrcpy, RustDesk, Moonlight/Apollo; Android 16/17, Windows 11, Linux portals). 20 ranked candidates, with sources; the main ones are quoted below.

The owner's standing rules hold: spec, then TDD; one logical change per commit; a fresh review per item, fixed one commit each; device checks on the A71 against this box (and the Azure VM for Linux); docs with each change; every feature's knobs in Settings on both sides; no macOS/iOS.

## Order

| # | Item | Size | Checkable here |
|---|---|---|---|
| U1 | The desktop app's shell: every page scrolls, grouped navigation, Save always reachable, one notice that clears, "Copied", screen readers, every script's glyphs, light-theme contrast, no frozen window | M | Yes (Windows) |
| U2 | The phone's navigation and feedback: Back, a card that says it is checking, plain errors everywhere, Lock and Ring confirmed and answered, wake results that announce themselves | M | Yes (A71) |
| U3 | Names on both sides: the computer's name on the phone, the phone's name on the computer, rename on both | M | Yes |
| N1 | Ring my phone from the computer, any time | S | Yes |
| N2 | Phone → computer text in one tap (the selection menu, Direct Share) | S | Yes |
| U4 | The phone's Home card and Settings: a Connect button, a status line, Settings in groups with one-line summaries, the session menu grouped, the stats overlay optional, TalkBack rows | M | Yes |
| U5 | The desktop's Settings in sections, plain words, one time format, empty states that lead somewhere, one word for the computer, one quality scale | M | Yes |
| U6 | Localization: hardcoded English to resources, Spanish and Tamil brought up | S | Yes (JVM, lint) |
| N3 | Answer a phone notification in the toast itself (Windows activation; Linux actions) | S–M | Yes (Windows); partly (VM) |
| N4 | An Android 17 pass (the app targets API 37): background audio, the code-text hold, the rest | S–M | Partly (an emulator image) |
| N5 | The phone as the computer's speaker and mic without a screen session | S–M | Yes |
| N6 | The desktop app's phone panel and a Linux tray | S–M | Yes (Windows); partly (VM) |
| N7 | The phone's background cost, measured and cut | S–M | Yes (A71) |
| N8 | GNOME and KDE hosts that work unattended (remembered portal grants, input through the portal) | M–L | Partly (a GNOME VM) |
| N9 | KDE Connect, next steps (notifications, shares both ways, beside `kdeconnectd`) | M | Yes (KDE Connect on the A71) |

Usability first: U1–U3 fix what stops or confuses a person today. N1 and N2 are small, asked for, and fill real gaps. Then the rest of the polish (U4–U6), then the larger features. Each item gets a short spec in `docs/superpowers/specs/` before code.

---

## U1. The desktop app's shell

**Audit findings:** D-L1 (no `ScrollArea` on Settings, Devices, Sessions, Pair, Diagnostics or Status; at the default 720×480 window Save and the Computers and KDE sections are off-screen, and Settings is about 1,500 px tall), D-L2 (12 tabs and the title in a row that does not wrap, about 900 px against 720), D-W1 (the notice line never clears, errors look like successes, copy buttons say nothing), D-W3 (two settings run PowerShell and `browser-host` on the UI thread, freezing the window 1–2 s), A1 (eframe built without AccessKit), A6 (hint, good and bad colours under 4.5:1 on the light panel), L1 (egui's default fonts only, so Tamil, CJK, Arabic and other scripts in notifications, texts and file names show as boxes).

**What:**
- A left navigation panel in groups (Overview; Phones and computers; Sharing: Files, Messages, Notifications, Phone files; Activity; Settings; Help: Diagnostics, Logs), the title above it. Each page in a vertical scroll area with a heading and one line of what it is for.
- Settings' Save and Undo in a bar at the bottom while the form has changes.
- One notice component: a kind (done, information, error) with its colour, gone after about 6 s or at the next action, with a close button; "Copied." after each copy.
- AccessKit on (eframe's `accesskit` feature).
- Fonts: the system's fallback fonts loaded at start (Windows: Nirmala UI, Segoe UI, Yu Gothic UI, Malgun Gothic; Linux: Noto through fontconfig).
- Colour tokens with light and dark values, chosen from the visuals.
- The two blocking settings run off the UI thread, as the app's other jobs do.
- Remember the window's size and the last page (eframe persistence).

**Tests:** the navigation model (which page each group holds), the notice's expiry, the font list found on each OS, a contrast check of each token pair (a pure function, WCAG ratio ≥ 4.5).

**Device check:** the app on screen on this box at 720×480 and maximized (a screenshot of the app's own window, no input into it: the window is resized by its own persisted size, and pages chosen by a start-up argument for the check, `--page`).

## U2. The phone's navigation and feedback

**Audit findings:** P-W1 (Back closes the app from Settings, Add computer, Scan, the pairing-link confirmation and Remote: the only `BackHandler` is in `RemoteScreen`), P-W2 (tapping a card waits seconds on an asleep check and an internet dial with no sign), P-W3 (raw Rust and OS errors in about 15 places; `humanizeError` used once), P-W4 (Lock and Ring with no confirmation and no result), P-W5 (the pairing sheet's primary button is the wrong one, and it prints a CLI command), P-W8 ("Exit" to leave a flow, a wake status that never clears and is not announced).

**What:**
- Back goes up one level everywhere; on Remote it ends the remote, as it ends a session (as built: neither asks).
- A card shows "Checking…" with a progress line while it decides, and ignores taps meanwhile.
- Every failure shown goes through one mapping to a plain, localized sentence, with the raw text in logcat and the copied report.
- Lock asks first ("Lock the computer?"; U3 names the computer); Ring and Lock say what happened; a failed shortcut says so.
- The pairing sheet: "Pair" primary, "Show a PIN on the computer" secondary, no command in the text, the eye toggling.
- "Cancel" to leave a flow, "Exit" only for a session; wake results in a snackbar.

**Tests:** the back-stack model (a pure route function), the error mapping (each known raw string to its resource), JVM.

**Device check:** Back from each screen, a failing connect's message, Lock's dialog (cancelled, never confirmed on this box).

## U3. Names on both sides

**Audit finding X1:** the phone lists computers by address; the computer's name comes in the pairing link and the identity and is thrown away. The desktop lists phones by a 12-character id everywhere; the phone's identity carries `deviceName`, which the trust store does not keep. Other computers and KDE devices already have names.

**What:**
- The phone keeps each computer's name (from the pairing link, then the identity at each connection) and shows it on cards, the session title, the notification and shortcuts, the address under it. Rename on the card's menu.
- The desktop keeps each phone's name (from its identity at pairing and each connection, sanitized and short) in the trust store, and shows it in Devices, the approval prompt ("Galaxy A71 wants to connect"), Sessions, Files, Messages, Notifications, the Phone tab and Activity, the id on hover. Rename in Devices (kept as the owner's choice, never replaced by the phone's own).
- `pocketdesk devices --json` gains `name` (the local API stays version 1: an added field).

**Security:** a phone's name is its own claim: shown as "calls itself" where a decision depends on it (the approval prompt shows the name and the id), sanitized like device ids (no control or invisible characters).

**Tests:** the trust store's name round trip and an old store without names, the sanitizer, the phone's `HostStore` with names and its migration.

## N1. Ring my phone from the computer, any time

**Research:** KDE bugs 479290 (find-my-phone does not ring when locked) and 402384 (not in Do Not Disturb); a KDE Discuss thread (July 2026) on an alarm stopped only by force-stopping. Pocketdesk rings KDE phones (3c) but not its own: the phone rings only from `SirenWatcher`, in a foreground session, and the server never pushes the ring.

**What:**
- The desktop app's Ring on each phone (Devices, and the phone panel later), `pocketdesk ring <phone>`, a Home Assistant button.
- The server sends `kdeconnect.findmydevice` over the phone's live link (the background link included), under a new per-phone permission "Ring this phone" (on by default for the owner's phones), at most once a minute per phone.
- The phone rings from its notification listener as well as in a session: full volume on the alarm stream, through Do Not Disturb when the user has granted it, a full-screen notification with Stop on the lock screen, stopping after 30 s.
- Android 17's background audio rule: ring from a short foreground service (or the alarm usage with its permission).

**Device check:** the A71 locked, in the background, DnD on: rings, stops from the lock screen, never more than a second or two in tests.

## N2. Phone → computer text in one tap

**Research:** KDE Discuss "automatic clipboard sync (or easier method)" (October 2026) and "match Phone Link's keyboard/clipboard" (November 2025); GSConnect #1167. Android 10+ blocks background clipboard reads; the selection menu and the share sheet need none.

**What:**
- `ACTION_PROCESS_TEXT`: select text in any app → "To computer": it goes to the computer's clipboard (the most recent, or a chooser with several).
- Saved computers as Direct Share targets (sharing shortcuts), so copy → Share → the computer is two taps.
- Text shared from the share sheet goes to the computer's clipboard (a link still opens as a link).
- A source's "sensitive" mark is kept: such text is not sent unless the user confirms.

**Device check:** select text in a browser on the A71, send, the computer's clipboard (the stand-in) holds it.

## U4. The phone's Home card and Settings

**Audit findings:** P-V1–P-V7, A2–A5: no visible Connect (the card connects when tapped, with six equal text buttons and Remove among them); ports as the subtitle; the add form unfinished; Settings rows indented unevenly and icons reused; 16 unrelated switches under "Session", summaries of 3–7 lines; the stats overlay always on; the session menu's value rows hiding their choices; rows two TalkBack stops each, headers not headings, the trackpad slider unnamed, small session controls.

**What:** a Connect button, Remote and Show screen secondary, the rest in an overflow menu; a status line (on this network, over the internet, asleep, last connected) instead of ports; the add form with a top bar and placeholders; Settings grouped (Session; Files and links; In the background; Notifications and texts; Automation; Pairing; Wake; Appearance; About) with one-line summaries and an ⓘ for detail; toggleable rows, headings, a named slider; "Show connection details" (off by default) for the stats overlay; the session menu in three groups with segmented choices; touch targets of 48 dp.

## U5. The desktop's Settings and words

**Audit findings:** D-L3, D-L4, D-W2, D-W4, D-W5, X2–X4: one flat grid of about 35 rows mixing everyday and developer choices and two apply rules; long text widening grids; empty states pointing nowhere (Files says no phone is in a session though linked phones and computers can take files); backticks and CLI names in the UI; three time formats; "desktop", "PC" and "host" beside "computer"; two quality scales; four ways to remove a pairing.

**What:** collapsing sections (Connection; Phones and sharing; Notifications and texts; Wake; Automation; Other computers; KDE Connect; Advanced), each change marked "applies now" or "after a restart"; wrapped text; empty states with the next step and a button; one `ago()`; "computer" for the machine on both apps; one quality scale; "Unpair" for pairings and "Remove" for saved items, each confirmed naming the target.

## U6. Localization

**Audit findings:** L2 (English hardcoded in the stats overlay, the remote's key captions, PhoneFiles' errors sent to the computer), L3 (long Spanish and Tamil labels in fixed rows). Spanish and Tamil brought up to every new string.

## N3. Answer a phone notification in the toast itself

**Research:** Phone Link's main notification feature; GSConnect lost it on GNOME 45. Pocketdesk answers only from the desktop app's Notifications tab (T10c).

**What:** on Windows, toasts with the notification's buttons and a reply box, through an activator for an unpackaged app (a COM server registered per user, as the AppUserModelID already is); on Linux, D-Bus actions (and an inline reply where the server offers it). The same rules as the tab: only what the notification offered, at most 10 a minute.

## N4. An Android 17 pass

**Research:** Android 17 (the app targets API 37) makes background audio and volume calls fail silently without a foreground service holding while-in-use capability (the siren), holds texts containing codes for 3 hours (forwarded texts), extends background launch limits to `IntentSender`, ignores orientation limits on large screens, no longer restores the keyboard after an unhandled configuration change, and reports a touchpad's motion as relative under pointer capture. An emulator image for 17 checks what the A71 cannot.

## N5. The phone as the computer's speaker and mic

**Research:** KDE's mic requests and bug 362990, scrcpy #4208, RustDesk's mic passthrough. Pocketdesk has sound both ways, but only inside a screen session.

**What:** a sound-only session mode (as T10i made input-only): no capture or video, the desktop's sound to the phone and the phone's mic to the computer, each under its permission; "Sound" on each saved computer.

## N6. The phone panel and a Linux tray

**Research:** Windows 11's Start menu phone panel; no tray on Linux (`tray_none.rs`).

**What:** the desktop app's Overview shows each phone: name, battery, linked or not, last session, and quick actions (Ring, Send file, Send link); a tray on Linux through StatusNotifierItem (`ksni`), with the same menu as Windows'.

## N7. The phone's background cost

Owed from R4 and R6: an overnight run on the A71 with each background feature on, the battery drop and the wake-ups, then cuts where they count.

## N8. GNOME and KDE hosts that work unattended

**Research:** `capture_linux.rs` asks the ScreenCast portal for a session each time, with no `persist_mode` or restore token, so a dialog appears at the desk at every session start and reconnect, and a session from away can never be approved. Input there needs uinput (root or a group `install.sh` does not set up). The RemoteDesktop portal with libei (`reis`, MIT) gives input without uinput. GNOME keeps restore tokens (xdg-desktop-portal-gnome MR 88); KDE's have been unreliable (KDE bug 517454).

**Security:** a remembered grant means sessions start with no one at the desk: an explicit owner choice in Settings, revocable, and said in SECURITY.md.

**Testing:** a GNOME VM on Azure (the budget rule: delete the resource group after).

## N9. KDE Connect, next steps

Notifications from KDE devices (as phones' are shown), shares both ways (sending needs a listener on 1739–1764 and its firewall rule), running beside KDE's own daemon on Linux (UDP 1716 shared), and a re-check against CVE-2025-32898 and CVE-2025-32900.

---

## Not now

From the research: a Quick Share receiver (security-heavy), second screens beyond Sway, controlling the phone through Accessibility (Android 17 Advanced Protection revokes it), the phone as a webcam, real touch and pen, photos into the computer, quiet hours, wider links (each fine later); calls (no SIM), unattended Windows sign-in, a Bluetooth transport, gamepad (ViGEmBus retired), TV and Wear (no hardware).

## Sources

The research notes with every link are summarised above; the main ones: Android 17 behaviour changes (developer.android.com/about/versions/17), background audio (…/17/changes/bg-audio); KDE bugs 479290, 402384, 362990; KDE Discuss threads on find-my-phone (July 2026) and clipboard (October 2026); GSConnect #1167; scrcpy #4208; xdg-desktop-portal-gnome MR 88; KDE bug 517454; `reis` (crates.io).
