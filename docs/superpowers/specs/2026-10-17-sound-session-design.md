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

Core: the mode's wire byte and that it gets no picture; a sound session keeps the mic's frames while a remote drops them. Server (`server/tests/sound_session.rs`, loopback QUIC): admitted, the mic reaching the relay, no input injected, no picture; refused with the reason with neither Audio nor Mic. Kotlin: a refusal for want of a permission names the switch (`PlainErrorTest`); otherwise the existing state machine, which the screen reuses.

## Check

On the A71 against this box (a throwaway server): a sound session admitted, the computer's sound playing on the phone (its player started, the AudioTrack playing), going on with the phone's screen off; the mic share started; the Sessions page saying "sound only"; refused with Audio and Mic off.

## As built

As designed, with these found on the way:

- **The mic never reached the computer (found on the device).** A session without a picture drops what is about the picture, and that filter took the mic's frames too, as is right for a remote. A sound session keeps them now (`about_the_stream(packet, sound)`), and the loopback test holds it.
- **A refusal read as "Something went wrong" (found on the device).** The close reason ("sound not allowed", and the remote's "input not allowed", which had the same gap since T10i) now becomes a sentence naming the switch on the computer's Devices page.
- **`MicShare`.** The screen session's mic share moved into `ui/MicShare.kt` (`rememberMicShare`), so both screens share the mic the same way, the permission prompt and the foreground service's restart after a grant included.
- **The notification** is the session's ("Streaming to …", with Disconnect and Lock desktop); Disconnect ends a sound session as it does a screen session, and Lock asks first, as in a remote.
- **What the computer allows.** A sound session's ready stream carries one byte (bit 7 set, bit 0 its sound sent, bit 1 the phone's mic played); the phone greys out the switch the computer would ignore and names the switch on the computer that turns it on. A computer that says nothing reads as both.
- **After a drop** it reconnects by itself as a screen session does (backoff, ten minutes; an end the computer chose, a refusal or a change of permissions, waits for the person), and once it stays down it stops its foreground service, so no wake lock is held for nothing.

**Fresh review (0 Critical, 3 Important, 5 Minor), fixed one commit each:** Retry never started the computer's sound again (the audio generation was keyed on composition, so the old attempt's Down ended the new one before it was admitted); the sound sheet dialed a connection per request (no control link); a drop kept the foreground service, a partial wake lock and the notification for hours; plus Lock desktop doing nothing, the computer's notification calling it "controlling this desktop", switches the computer would ignore, "Give control" offered to a session that gives no input (a cast too), and two stale docs. Found on the re-check: a session ended by a change of permissions read as "Something went wrong" in every session kind (now said).

Checked on the A71 against this box (a throwaway server, the internet road): admitted, "sound only" in `status --json` and the mirror; the computer's sound decoded and playing (`AudioTrack` started, not muted) with the phone's screen off and no background-playback warning; the mic share's frames reaching the relay (this box has no virtual cable, so the relay said so); refused with Audio and Mic off, the phone naming the switches; Disconnect from the notification ending it on both ends. Again after the review fixes: Mic off greyed the mic switch with its reason; the computer's notification said "a phone is this computer's speaker and mic"; the server stopped, the phone noticed in about 20 s (QUIC's idle timeout), counted down and came back by itself with the sound playing again; Lock desktop from the notification asked (cancelled; never confirmed on this box); a change of permissions ended it with that reason, no retry, and the service and notification gone. Owed: by ear (lag, quality), a virtual cable on Windows, the Linux host (`pw-loopback`), a call on the computer through the phone's mic, giving up after ten minutes.
