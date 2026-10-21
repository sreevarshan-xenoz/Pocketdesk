# M7: the phone's sound without its screen (K5 part 2)

Plan: `docs/research/2026-10-21-m-cycle.md`, M7. K5 part 1 (`2026-10-20-phone-sound-design.md`) sends the phone's sound only with its screen.

## Why

- **Asked for:** scrcpy's `--no-video`, SoundDrift and AudioRelay, which do only this.
- **Left over:** K5 part 1 left it out because the desktop opens the cast's window as a cast starts.

## What

- **On the phone:** "Play this phone's sound there" in each computer's menu on Home, beside "Show camera". It needs:
  - Android 10 or newer;
  - the microphone permission, which Android asks for to capture other apps' sound.

  It asks for the microphone first, then Android's screen-capture consent, as "Show screen" does, saying first: "Android calls this screen sharing; only the sound is sent." Then a foreground service captures what the phone's apps play (K5's `CastSound`, under the projection) and sends it with no encoder and no picture. Its notification says "This phone's sound plays on Ann's computer", with Stop. A capture that ends by itself ends the session, and a start while the screen or the camera is shown is refused before the consent.
- **The road:** a session of its own mode, `4` (`SESSION_MODE_PHONE_SOUND`), on the session mode stream. Its sound goes as K5's cast streams (`[0xFA, 0x03]` and the Opus packet).
- **On the computer:**
  - **Permission:** the phone's "Play its sound here" (K5's `PhoneSound`), not "Show its screen or camera here", since no picture comes. Without it, the session is refused with "phone sound not allowed".
  - **No player:** with no sound player on this computer, it is refused with "phone sound not played here".
  - **Playing:** K5's player plays it. No input is taken, and the cast relay is never told, so no window opens.
  - **The ready stream:** says the sound is taken (K5's feature bit), as a cast's does.
  - **Where it shows:** the Sessions page lists it as "its sound here", with End and never Give control; `pocketdesk status` says "playing its sound"; its notification says "a phone is playing its sound here", and Activity "Playing its sound here".
  - **Turned off mid-session:** the permission ends it, as for a cast's sound (K5).

## Left out

- The phone's own speaker keeps playing (K5's limit).
- Calls, and apps that forbid capture, stay silent (K5's limits).
- A quick-settings tile for it.

## Privacy and safety

- **Consent:** Android's own consent dialog, each time, names it screen sharing. The phone's line before it says only sound is sent.
- **What goes:** only the sound that apps allow to be captured (Android's rule). No picture is encoded or sent.
- **Who plays it:** only a computer that allows it; it is off by default for each phone (K5's permission).

## Tests

- **Core:** the mode's wire byte and claim; a phone-sound session counts as no picture and no input.
- **Server (loopback, `server/tests/cast_sound.rs`):**
  - a phone-sound session's sound reaches the player with the permission, and the cast relay hears nothing;
  - without the permission it is refused, with its reason.
- **Mirror:** the session is "its sound here", not "sound only".
- **Phone (JVM):** when the menu offers it (Android 10+); the rule that starts capture (K5's `CastSoundRule`, with the computer's word).

## On the device (checklist §172)

- The OPPO with the throwaway tone app against a throwaway server, through the WAV seam (`POCKETDESK_PHONE_AUDIO_FILE`): the tone's frequency and level; no cast window (the relay's log).
- Refused without the permission, with the reason on the phone.
- Stop from the notification.
- An hour's battery with the screen off.
- By ear, and a real music app (the owner).

## As built

- **Core:** `SESSION_MODE_PHONE_SOUND` and `SessionMode::PhoneSound`; `RemoteFlags.phone_sound` (and `cast`, since the phone sends its stream); the streamer's `phone_sound_only`, refused without `PhoneSound` or a player, its sound to K5's player, no input and no relay; `MirrorSession.phone_sound_only`.
- **Server:** the mirror, `status`, `status --json` (`phone_sound_only`), the `session_started` mode `phone_sound`, and the session's notification.
- **Desktop:** "its sound here" on the Sessions page, never offered control; Activity's line.
- **Bridge:** the cast connect's `sound_alone`; `cast::start` and `stop_cast` for both modes.
- **Phone:** `CastService` with `soundAlone` (no encoder, display or control; ends when `CastSound.alive` goes false); `CastSoundRule.offeredAlone`; Home's menu item; `MainActivity`'s microphone ask, then the consent; strings in English, Spanish and Tamil.

## Review

A fresh review found 0 Critical, 3 Important and 4 Minor. Each fix is its own commit by side.

- **Important:**
  - a capture that died left a silent session up until Stop;
  - the computer's notification called it its speaker and mic, and Activity read it as viewing the computer;
  - on Android 13+ the notifications' request cancelled the microphone's, so the feature could not be reached.
- **Minor, fixed:**
  - Give control offered for it;
  - the phone's titles speaking of the screen;
  - a second start dropped without a word;
  - the claim and the permission's end untested, and no checklist section.
