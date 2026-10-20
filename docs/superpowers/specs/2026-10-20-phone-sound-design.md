# The phone's sound on the computer (K5, part 1)

**Date:** 2026-10-20. **Branch:** `feat/phone-sound`. The K cycle's fifth item (`docs/research/2026-10-20-k-cycle.md`).

## What is wrong

- **"Show screen" is silent.** The phone's screen reaches the computer (T10k), but a video, a game or a call's ringtone on the phone plays only on the phone.
- **Asked for:** scrcpy 2.0's audio forwarding; Phone Link's "hear audio on PC"; whole apps for it (SoundDrift, AudioRelay).

## What changes

- **The phone:** with Settings "The phone's sound with its screen" on (off by default; it asks for the microphone permission, which Android requires to capture other apps' sound), "Show screen" also captures what the phone's apps play.
  - **Capture:** `AudioPlaybackCapture` (Android 10+) under the projection the cast already holds, for media, games and other sound apps let others capture.
  - **Encoding:** Opus at 48 kHz stereo, 96 kbit/s, 20 ms frames, with the encoder E2's mic share uses.
  - **Sending:** on the cast session, as a cast stream of its own (`[0xFA, 0x03]` and the Opus packet).
- **The computer:**
  - **Permission:** a per-phone permission, "Play its sound here" (off by default, Devices page).
  - **Playing:** with the permission on, the phone's sound plays on the computer's default output: WASAPI on Windows, `pw-cat` on Linux. A test seam (`POCKETDESK_PHONE_AUDIO_FILE`) writes it to a WAV file instead.
  - **What the phone is told:** the cast's ready stream says whether its sound is taken (a feature bit), so a phone whose computer would drop it does not capture.
  - **Turned off mid-cast:** the cast's sound stops at once (the session goes on).

## Left out

- **Sound without the picture** ("Play the phone's sound here" alone): the desktop opens the cast's window as a cast starts, so it needs a cast with no window. Part 2.
- **Silencing the phone's own speaker:** capture takes what plays; Android has no way for another app to mute the speaker while capturing. The owner turns the phone's volume down (the capture is not affected on Android 10+ for media).
- **A/V sync:** the sound is not timed to the picture (roadmap 2794); measured on the device.
- **Calls:** Android never lets another app capture call audio.
- **Apps that forbid capture** (`ALLOW_CAPTURE_BY_NONE`): silent; the phone cannot tell from its side.

## Testing

- **Unit:**
  - the cast stream's audio framing (parse, bytes, size limit);
  - the permission's default and the feature bit;
  - the WAV seam's header;
  - the phone's rule for when to capture (JVM: the setting, the permission, the computer's word, Android 10+).
- **Server:** a cast session over loopback whose audio streams decode into the WAV seam with the permission on, and nothing with it off (`server/tests/cast_session.rs`).
- **On the device (checklist §155):** a throwaway app plays a 1 kHz tone; the WAV seam on a throwaway server checks its frequency and level; by ear, a real video.
