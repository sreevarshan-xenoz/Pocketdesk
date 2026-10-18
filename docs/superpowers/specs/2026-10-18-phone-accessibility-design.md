# The phone with TalkBack, large fonts, landscape and split screen (C5)

**Plan:** `docs/research/2026-10-17-next-cycle.md`, C5.

## What was found

On the A71 (Android 16, a 1080×2400 screen at 420 dpi):
- **Labels and touch targets:** every clickable or checkable node on Home, the Add form (with its advanced part), Settings, the session and the session menu has a label, and is at least 48 dp each way. The audit read uiautomator's tree, which is the tree TalkBack reads. Three nodes flagged at first were rows cut by a scroll edge; Material's own bottom-sheet drag handle is 32 dp wide.
- **Font scale 1.3:** Home, Settings, the session and its menu fit.
- **Font scale 2.0:**
  - Home and Settings fit: rows wrap, and summaries stop at two lines behind their ⓘ, as built (U4).
  - **The session menu's choices break words.** The Quality row reads "Balanc / ed" and "Econo / my": a segment is a quarter of the sheet, narrower than one word.
  - **The session's chrome overlaps.** Over a phone's portrait stream pane (a quarter to 60 % of the screen, T9b), the chips at 2.0 run into each other: the connection, the windows and the mode chip, with the screen chips over them.
- **Landscape:** Home is fine. **A session splits into two panes**: the stream on the left half and the trackpad on the right. Pane "auto" splits any window 600 dp wide (E6), and a phone held sideways is about 914 dp wide, so its stream gets half the screen.

## Design

- **Choices that do not fit become a list.** A row of choices in the session menu measures the longest word of each option at the face it would use. If any is wider than its segment, the row is drawn as radio buttons, one per line, with the same TalkBack roles (`ChoiceFit`, JVM-tested).
- **The chrome's text stops at 1.3×.** The session's chrome over the stream (its top and bottom groups) is drawn at the font scale capped at 1.3 (`ChromeText`). The session menu, sheets and every other screen keep the full scale. The chrome is a few words over moving video, in a pane that can be a quarter of the screen.
- **A phone held sideways keeps one pane.** Pane "auto" splits side by side only when the device's smallest width is also 600 dp (a tablet, or an unfolded phone): `PaneRule.autoSplit` takes it. "Pane: Dual" still splits on request.

## Testing

- **JVM:** `ChoiceFit`, `ChromeText` and the new `PaneRule` cases.
- **On the A71:** the session menu at 2.0, the chrome at 2.0, a session in landscape.
- **TalkBack:** turned on with `settings put secure enabled_accessibility_services`, and a walk through Home, the Add form, Settings and the session menu by swipes, restored after.
- **Instrumented:** a Compose test of the choice row at font scale 2.0 (`connectedDebugAndroidTest`, with `leaveApksInstalledAfterRun` so the app keeps its pairings).

## Out of scope

- Split screen beyond a check that a session and Home draw in half a screen.
- TalkBack in Spanish and Tamil.
