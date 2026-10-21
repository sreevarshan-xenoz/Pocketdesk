# M5: photos keep where they were taken

Plan: `docs/research/2026-10-21-m-cycle.md`, M5.

## Why

A photo sent from the phone arrives without its GPS tags. Since Android 10, an app reading another app's photo from MediaStore gets it with the location taken out, unless it holds `ACCESS_MEDIA_LOCATION` and asks for the original (`MediaStore.setRequireOriginal`). Pocketdesk does neither.

KDE bug 421613 ("Geotagged pictures lose GPS data after wifi transfer") is still open; the /e/ forum traced it to this permission, which LocalSend holds.

## What

- **The phone's Settings:** "Photos keep where they were taken", under the photos, off by default, shown on Android 10 and newer only (older ones take nothing out). Turning it on asks for photo access and `ACCESS_MEDIA_LOCATION` together: Android grants the media location only to an app with access to all photos, and refuses it without a word when asked alone. With it off, nothing changes, and the row says Android takes the place out.
- **What it changes:** with it on and both granted, the transfer service opens each file it sends through `MediaStore.setRequireOriginal` (`PhotoPlaceRule.route`):
  - **A MediaStore item** (the `media` authority) is asked for whole.
  - **A picked file** (the Files app's `com.android.providers.media.documents`, or, on Android 11+, external storage's documents) is first mapped to its MediaStore item (`MediaStore.getMediaUri`).
  - **Anything else** is opened as named; a cloud gallery's provider (Google Photos) decides for itself.

  Every road sends through the transfer service, so this covers a share, a folder send's files, a recent photo the computer fetches (C7), a new screenshot (T10h) and a file the computer's drive asks for (D8).
- **If Android refuses the original:** for example, a share's grant may cover only the item it named. The photo is then opened as named and goes without its place, as before, and the log says so (`firstThatOpens`). Only those refusals fall back; any other error, a cancel among them, is the send's own.
- **Resume:** the original and the place-less copy differ in their bytes, so a resume is keyed on which was opened, and never mixes the two.
- **If photo access is taken away later,** the row says access is needed, and photos go as before.

## Not now

- The Health card for revoked access: the row says it.
- The computer seeing the place (a map): the file keeps its tags, and the computer's own viewers show them.

## Privacy

A photo's place is where someone was. It goes only with the switch on, only to the phone's own computers, and only in a photo the person chose to send, or one the computer was allowed to fetch. SECURITY.md limit 41.

## Tests

JVM: `PhotoPlaceRule.route` (the switch, access, Android 9, the authorities, picked files from Android 10 and 11) and `firstThatOpens` (the original first, each refusal's fallback, a cancel not caught).

## On the device (checklist §169)

- A JPEG with GPS tags pushed to `DCIM/Camera` by adb, shared to a throwaway server with the switch on: the tags arrive, read with an EXIF reader.
- The same with the switch off: the tags are gone.
- Fetched through C7 with the switch on: the tags kept.

## As built

- `files/PhotoPlace.kt`: `PhotoPlaceRule` (`route`, `firstThatOpens`) and `PhotoPlacePrefs` (`granted`, `permissions`, `open`).
- `service/TransferService.kt`: `sendOne` and `sendCopy` open through it; `sourceKey`.
- `ui/PhotoPlaceSettings.kt`: the row.
- The manifest's `ACCESS_MEDIA_LOCATION`, and strings in English, Spanish and Tamil.

## Review

A fresh review found 1 Critical, 3 Important and 6 Minor, fixed in two commits: the code's findings together, since they share the same lines, and the words.

- **Critical:** the media location was asked alone, which Android denies without a word to an app without photo access. The switch snapped back.
- **Important:**
  - the spec claimed folder sends and the drive, whose picked files are never MediaStore items: they are mapped now;
  - the fallback stripped the place silently: it is logged, and full photo access is required;
  - the fallback caught everything, a cancel too: only Android's refusals now.
- **Minor, fixed:**
  - a resume mixing the original and the copy;
  - the row naming a permission no Settings page shows;
  - Android 9;
  - the translations' words;
  - tests for the fallback.
- **Left:** the review's reading of Android's permission dialogs is from AOSP's code, not from a device. Checklist §169 starts with photo access revoked.

