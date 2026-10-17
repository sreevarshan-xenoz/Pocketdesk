# The phone as the computer's speaker and mic, without the screen (N5)

Date: 2026-10-17. Plan: `docs/research/2026-10-17-finish-and-polish.md` §N5. Research: KDE Connect's requests for mic and speaker modes and bug 362990, scrcpy #4208, RustDesk's mic passthrough.

## The problem

Pocketdesk carries sound both ways (the computer's sound on the phone, 2026-10-12; the phone's mic into the computer, R4 E2), but only inside a screen session, which captures, encodes and sends video the whole time. Using the phone as a speaker across the room, or its mic for a call on the computer, costs a screen session's CPU and battery and keeps the phone's screen busy.

## The design

1. **A sound session mode** (`SESSION_MODE_SOUND = 3` on the T10i mode stream, `SessionMode::Sound`): the computer starts no capture, encode, video or bitrate arbiter, injects no input (it drops input packets as it does a view-only session's), plays the phone's mic into its mic relay when the phone has Mic, and sends its sound when the phone has Audio. A phone with neither is refused ("sound not allowed"). It says when it is admitted with the ready stream, as a remote does.
2. **On the computer:** the live session is listed as "sound only" (Sessions page, `status`), and its event's mode is `sound`.
3. **On the phone:** "Sound" in each saved computer's ⋮ menu opens a small screen: the state (connecting, waiting for the computer, on), "The computer's sound here" (the existing "Desktop sound on this phone" switch), "This phone's mic there" (the existing mic share), and the computer's volume, mute and output (the existing sound sheet). It runs under the session's foreground service, so it goes on with the screen off or the app in the background, and the notification ends it.
4. **Bridge:** `nativeConnectSound` / `nativeConnectSoundWan` (the remote's pair, with the sound mode); the remote's state works for it.

## Tests

Core: the mode's wire byte and that it gets no picture. Server: a sound client says so and asks for no picture; a phone with neither Audio nor Mic is refused; one with Audio gets sound and no video (a loopback session, as `input_only_session.rs`). Kotlin: none beyond the existing state machine, which the screen reuses.

## Check

On the A71 against this box (a throwaway server): a sound session admitted, the computer's sound playing on the phone (its player started, the AudioTrack playing), going on with the phone's screen off; the mic share started; the Sessions page saying "sound only"; refused with Audio and Mic off.
