# O2, a screenshot of the phone and its screen locked from the computer (design)

Date: 2026-10-22. Plan: `docs/research/2026-10-22-o-cycle.md` §O2. Asked for in scrcpy #6682 ("Is it possible to get/trigger a screenshot?") and #6426: today a screenshot of the phone needs its screen shown in a window (L5).

## What

"Screenshot of the phone" and "Lock the phone" on the Overview and in the tray's menu, for a linked phone.

## The road, as L4's numbers to dial

- **The computer.** The desktop writes `phone_actions_out/<id>.json` (`core::local::phone_actions`). The server takes it and sends `kdeconnect.pocketdesk.phone_action {action}` (`screenshot` or `lock`) to that phone if it is connected within a minute. It goes only to a phone still paired, with its new per-phone permission "Take screenshots of it and lock it from here" (`Capability::PhoneActions`, on).
- **The bridge.** It keeps the ask with the computer it came from (`phone_actions.rs`). A link that ends forgets it.
- **The phone.** The notification listener takes the asks on its tick. `PhoneActions` does each through C8's control service, with `GLOBAL_ACTION_TAKE_SCREENSHOT` or `GLOBAL_ACTION_LOCK_SCREEN`, when `PhoneActionRule` (JVM-tested) allows:
  - the phone's "Let the computer take screenshots and lock this phone" is on (off by default);
  - the ask comes from its own computer;
  - the service is on, on Android 9 or later;
  - at most 10 a minute;
  - for a screenshot, also: the keyguard is not showing, the screen is on, and T10h's "Send new screenshots" is on, since that is what carries the picture to the computer.
- **The link.** With the switch on and the service up, the listener keeps the own computer's link, as M4's typing does.

L7's tables gain `phone_actions`, so the Devices page and the phone's mirror line up both sides.

## Not done

- **Nothing comes back to the computer** but the screenshot itself. The Overview says what it asked; a refusal is logged on the phone.
- **No screenshot of the lock screen,** and no unlocking: Android gives no app an unlock.

## As built and checked

Checked on the OPPO (CPH2359) against a throwaway server through `adb reverse`, the requests written as the desktop app writes them (checklist §185):
- **A screenshot:** taken and saved by ColorOS's screenshot service, then picked up by T10h. Its delivery was refused only because the server will not dial back to 127.0.0.1 over `adb reverse`, so the arrival is owed on the LAN road.
- **A lock:** the keyguard shown, the phone asleep.
- **Refusals:**
  - a screenshot on the lock screen;
  - a screenshot with the screen off;
  - a lock with the computer's permission off ("A lock was not asked").

Found there and fixed before the commit:
- **The screen off.** With the screen off but the phone not yet locked, the service said it took the screenshot, and Android logged "Attempted to take screenshot while display was off": no picture. The screen must now be on (`ASLEEP`).
- **The keyguard.** A phone without a PIN reports `isDeviceLocked` false with the keyguard up, so the lock-screen check now reads `isKeyguardLocked`.
- **Android's Accessibility page** lists what the service does: the new actions are named there.
