# Pocketdesk: send to phone

A browser extension that adds three items to the right-click menu: **Send page to phone**, **Send link to phone** and **Send image to phone**. It hands each to Pocketdesk on this computer, which sends it to your phone:
- a link arrives as a notification that opens it;
- an image arrives as a file in the phone's Downloads.

It talks only to the Pocketdesk program on this computer, through the browser's native messaging. It has no network access of its own and no listener, and it reads nothing from pages but what you right-click.

## Set up

1. **Register Pocketdesk with your browsers.** Run `pocketdesk browser-host --install`, or switch on Settings, "Send from the browser", in the Pocketdesk desktop app. This covers Chrome, Chromium, Edge, Brave, Vivaldi and Firefox. On Linux, only browsers whose settings folder exists are registered.
2. **Load this folder as an extension:**
   - **Chromium browsers:** open `chrome://extensions`, turn on Developer mode, choose "Load unpacked" and pick this `browser/` folder. The extension's id is fixed by the `key` in `manifest.json` (`abojofjjnpcmejhdonndlegfaolfkcif`), which is the id the registration allows.
   - **Firefox 121 or later:** open `about:debugging#/runtime/this-firefox`, choose "Load Temporary Add-on…" and pick `manifest.json`. Its id is `pocketdesk@pocketdesk.dev`.

`pocketdesk browser-host --remove` undoes step 1.

## Which phone

- **Links** go to the first phone in a session with this computer whose Links permission is on, since a link is shown on the phone during a session.
- **Images** go to the phone files reach now: the one in a session first, else one connected at home, as "Send to phone" in the file manager picks.

Pocketdesk downloads the image itself, never more than 32 MB. Each phone's Links and Files permissions apply, as for any link or file.

## Limits

- **Snap and Flatpak browsers** read their hosts from other folders and may not start a program outside their sandbox, so the extension cannot reach Pocketdesk from them. Use a browser installed from your distribution's packages, or the vendor's own package.
- **If Pocketdesk moves** (a new folder, or a package that installs it elsewhere), run `pocketdesk browser-host --install` again: the registration names the program's path.
- **Images are downloaded by Pocketdesk, not the browser**, without your cookies: one behind a login or a site's hotlink protection fails with what the site answered (403, say). Save it and send the file instead. Only PNG, JPEG, GIF, WebP, AVIF and BMP are sent, named for their type.
- **On Windows** the host is registered for every browser it knows, installed or not, so one installed later finds it too.
