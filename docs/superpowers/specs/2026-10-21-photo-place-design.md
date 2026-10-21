# M5: photos keep where they were taken

Plan: `docs/research/2026-10-21-m-cycle.md`, M5.

## Why

A photo sent from the phone arrives without its GPS tags. Since Android 10, an app reading another app's photo from MediaStore gets it with the location taken out, unless it holds `ACCESS_MEDIA_LOCATION` and asks for the original (`MediaStore.setRequireOriginal`). Pocketdesk does neither.

KDE bug 421613 ("Geotagged pictures lose GPS data after wifi transfer") is still open; the /e/ forum traced it to this permission, which LocalSend holds.

## What

- **The phone's Settings:** "Photos keep where they were taken", in the files group, off by default. Turning it on asks for `ACCESS_MEDIA_LOCATION` ("Allow Pocketdesk to access your media's location"). With it off, nothing changes, and the row says Android takes the place out.
- **What it changes:** with it on and granted, every file the phone sends is opened through `MediaStore.setRequireOriginal` when it is a MediaStore item (the `media` authority, Android 10+). Every road sends through the transfer service, so this covers:
  - a share from the share sheet;
  - a folder send's files;
  - a recent photo the computer fetches (C7);
  - a new screenshot (T10h);
  - a file the computer's drive asks for (D8).
- **Other providers decide for themselves.** A picked folder (Android's folder picker) or a cloud gallery's provider (Google Photos) may still take the place out. The row says "when the photo is on this phone".
- **If the permission is taken away later,** the row says so, and photos go as before.

## Not now

- The Health card for a revoked permission: the row says it.
- The computer seeing the place (a map): the file keeps its tags, and the computer's own viewers show them.

## Privacy

A photo's place is where someone was. It goes only with the switch on, only to the phone's own computers, and only in a photo the person chose to send, or one the computer was allowed to fetch. SECURITY.md limit 41.

## Tests

JVM: `PhotoPlaceRule.asksOriginal`, covering the switch, the permission, Android 9 and the authority.

## On the device (checklist §169)

- A JPEG with GPS tags pushed to `DCIM/Camera` by adb, shared to a throwaway server with the switch on: the tags arrive, read with an EXIF reader.
- The same with the switch off: the tags are gone.
- Fetched through C7 with the switch on: the tags kept.
