# The phone's navigation and feedback (U2)

Date: 2026-10-17. Plan: `docs/research/2026-10-17-finish-and-polish.md` §U2. From the UI audit: P-W1–P-W5, P-W8.

## The problem

- **Back closes the app.** Settings, Add computer, the QR scanner, the pairing-link confirmation and the remote have no `BackHandler` (the session has one), so Android's Back leaves the app, and on the remote it ends the remote with it. Seen on the A71.
- **A tap that seems dead.** With "Offer to wake" on (the default), tapping a computer first checks whether it is asleep and tries the internet road, which can take seconds, with nothing on screen.
- **Raw errors.** About fifteen places show the bridge's own text ("Connection failed: … (os error 111)") where `humanizeError`, used in one place, would say it plainly.
- **Lock and Ring say nothing.** Lock locks the computer at one tap, with no question and no result; Ring and the shortcut keys drop their failures.
- **The pairing sheet.** Its main button is the instruction "Enter PIN from desktop", outlined, while "Show PIN on desktop" is the filled one; a line shows a command in backticks; the eye says "Show PIN" whether it shows or hides.
- **"Exit" to leave a flow, and a wake status that never clears.** The scanner and the pairing confirmation say "Exit", the session's word; after Wake a line stays at the bottom of Home, never announced.

## The design

1. **Back goes up one level:** from Settings, Add computer and the scanner to Home; from the pairing confirmation, cancelling it; from the remote, ending it as the session's Back does. A small `Navigation.backTo(route)` holds where each goes (JVM-tested).
2. **"Checking…" on the card** tapped while it decides whether the computer is asleep, with a thin progress line; taps on it are ignored until the check ends.
3. **Plain errors everywhere.** One function decides what a person sees for a failure: the bridge's technical text (an OS error, a TLS or QUIC failure, a timeout) becomes the plain sentence `humanizeError` gives, and the raw text goes to logcat; a sentence the computer wrote for a person (a refusal with its reason, "This file is hidden") is kept as it is. Every place listed in the audit goes through it. Its rules are JVM-tested.
4. **Lock asks first** ("Lock <computer>?"), and Lock and Ring say what happened in a snackbar ("Locked", "Ringing", or why not); a shortcut key that fails says so.
5. **The pairing sheet:** "Pair" is the filled button once a PIN is typed, "Show a PIN on the computer" the second; no command in the text; the eye says "Show PIN" or "Hide PIN" and changes.
6. **"Cancel" to leave a flow,** "Exit" only to end a session; wake results in a snackbar, which is announced and goes by itself.

## Tests

`Navigation.backTo`; the plain-error rules over the bridge's real messages and the computer's refusals.

## Device check

On the A71: Back from each screen; a card's "Checking…" against a computer that does not answer; a failed connect's message; Lock's question (cancelled, never confirmed on this box); a wake's snackbar.
