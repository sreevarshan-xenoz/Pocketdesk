# M8: `tel:` links and Call back (L4 part 2)

Plan: `docs/research/2026-10-21-m-cycle.md`, M8. L4 (`2026-10-21-dial-design.md`) dials a number from the Messages page or `pocketdesk call`.

## Why

- **A phone number on a web page or in an email does nothing on the computer.** KDE Connect's own `tel:` handler is asked for in KDE bugs 341950, 397333 and 410149 (2026-06-13).
- **Calling back means typing the number on the phone.** A ringing call's note (F5) offers no way to call back.

## What

- **A `tel:` link opens on the phone.**
  - **Windows:** desktop Settings "Open phone numbers on the phone" (off) registers Pocketdesk for `tel:` and `callto:` under HKCU:
    - a ProgID, `Pocketdesk.tel`, whose `shell\open\command` runs `pocketdesk.exe tel "%1"`;
    - `Software\Pocketdesk\Capabilities\URLAssociations`;
    - `RegisteredApplications`.

    Windows then asks which app opens the link, or the owner picks Pocketdesk in Default apps. Windows lets no app make itself the default.
  - **Linux:** a desktop file, `pocketdesk-tel.desktop` (`x-scheme-handler/tel` and `callto`), made the default with `xdg-mime`.
  - **Both:** `pocketdesk tel-handler [--remove]` does the same; Windows' `uninstall` removes it. A marker in the config folder (`tel_handler.json`) tells the desktop's row it is on.
- **`pocketdesk tel <uri>`** takes the number from a `tel:` or `callto:` link:
  - **Parsing:** the scheme (any case, `//` after `callto:`) is dropped, percent-escapes are decoded, and L4's rule cleans the number. A link with parameters (`;ext=`, `;phone-context=`) or a query is refused. L4's rule refuses `*` and `#`, so a link cannot dial a code.
  - **Which phone:**
    - one paired phone that allows "Call numbers from here" and is linked now: there;
    - else one such phone at all: there;
    - several, or several linked: the desktop app opens and asks on the Overview, "Call +1 555 0100 on:", with a button each and Cancel. The question goes after two minutes.
  - **Reported:** in a desktop notification, since a browser shows no console.
- **Call back:** a ringing call's note (F5) on the Notifications page has **Call back** when its title names a number. It goes through L4's road to the phone the call rang on.

## Not now

- Call back on Activity's lines.
- Settling a tie by the phone last used.

## Privacy and safety

- **The registry writes are HKCU only,** offered only from Settings or the command, and undone by the switch and by `uninstall`.
- **A web page can offer a `tel:` link.** The phone only opens its dialer with the number; the owner presses Call (L4's rule). L4's rule refuses codes.
- **The question names numbers on the computer's screen,** as the Messages page does.

## Tests

- **Core:** `tel:` and `callto:` parsing (escapes, separators, parameters, codes, case); the question's file (kept, read, expired, taken).
- **Server:**
  - the phone chosen from several, linked or not, with and without the permission;
  - the registry keys written, read back and removed (Windows, ignored without a session);
  - the desktop file's text.
- **Desktop:** the number from a call note's title; the question's buttons (harness).

## On the device (checklist §173)

- `pocketdesk tel tel:+1-555-0100` to the OPPO through a throwaway server: the dialer opens (`dumpsys activity activities`).
- A `tel:` link clicked in a browser after picking Pocketdesk in Default apps (the owner).
- Call back from a ringing call's note (a SIM).
