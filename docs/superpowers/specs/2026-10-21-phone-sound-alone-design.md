# M7: the phone's sound without its screen (K5 part 2)

Plan: `docs/research/2026-10-21-m-cycle.md`, M7. K5 part 1 (`2026-10-20-phone-sound-design.md`) sends the phone's sound only with its screen.

## Why

- **Asked for:** scrcpy's `--no-video`, SoundDrift and AudioRelay, which do only this.
- **Left over:** K5 part 1 left it out because the desktop opens the cast's window as a cast starts.

## What

- **On the phone:** "Play its sound on the computer" in each computer's menu on Home, beside "Show screen". It needs:
  - Android 10 or newer;
  - the microphone permission, which Android asks for to capture other apps' sound.

  It asks Android's screen-capture consent, as "Show screen" does, and says first: "Android calls this screen sharing; only the sound is sent." Then a foreground service captures what the phone's apps play (K5's `CastSound`, under the projection) and sends it with no encoder and no picture. Its notification says "Playing this phone's sound on Ann's computer", with Stop.
- **The road:** a session of its own mode, `4` (`SESSION_MODE_PHONE_SOUND`), on the session mode stream. Its sound goes as K5's cast streams (`[0xFA, 0x03]` and the Opus packet).
- **On the computer:**
  - **Permission:** the phone's "Play its sound here" (K5's `PhoneSound`), not "Show its screen or camera here", since no picture comes. Without it, the session is refused with "phone sound not allowed".
  - **No player:** with no sound player on this computer, it is refused with "phone sound not played here".
  - **Playing:** K5's player plays it. No input is taken, and the cast relay is never told, so no window opens.
  - **The ready stream:** says the sound is taken (K5's feature bit), as a cast's does.
  - **Where it shows:** the Sessions page lists it as "its sound here", with End; `pocketdesk status` says "playing its sound".
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
