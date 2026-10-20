# Files keep their dates (K1)

**Date:** 2026-10-20. **Branch:** `feat/file-dates`. The K cycle's first item (`docs/research/2026-10-20-k-cycle.md`).

## What is wrong

- **Every received file is dated when it landed.** A photo taken in June, sent today, shows as today's in Explorer and in the phone's Files app, and sorts with today's downloads.
- **No road carries the date:**
  - phone → computer shares and folders;
  - computer → phone offers and folders;
  - computer ↔ computer;
  - KDE devices' shares, whose packet carries `lastModified` and is ignored.
- **The same complaint elsewhere:** KDE bug 368013/503297, open since 2016; LocalSend #650, reopened in 2026.

## What changes

- **The wire.** The share request and the offer carry an optional `lastModified`: the file's modified time in milliseconds since 1970, as KDE Connect's share packet has it. An end that does not know it ignores it, and an end that gets none dates the file as before.
- **The rule, one place** (`core::protocol::file_time`):
  - a date of 0 or less, or one that does not parse, is none;
  - a date after now is now;
  - it is applied only once the file is whole and in its place.
- **The computer, receiving** (shares from phones and computers, offers fetched from another computer, KDE devices' shares): after the file is moved into place and marked as downloaded, its modified time is set (`std::fs::File::set_modified`). A failure to set it is logged; the file is kept.
- **The computer, sending** (offers to phones, folder sends, shares to another computer): each file's own modified time goes with it.
- **The phone, sending** (shares, folders, screenshots): the date the provider gives, `DocumentsContract`'s `COLUMN_LAST_MODIFIED` or MediaStore's `DATE_MODIFIED`.
- **The phone, receiving** into Downloads/Pocketdesk, from an offer or the remote file browser (the listing's date): the date is set on the file the app wrote through its path, or, where Android will not allow that, on MediaStore's own date. A folder picked with Android's picker (C2) has no way to set a date, and Settings says so.
- **Settings "Keep files' dates"** on both sides, on by default. Off, nothing is sent and nothing is set.

## Left out

- The creation time: Windows has one and Android does not, so it would be half kept.
- Dates on the clipboard's images and on photos fetched for the Phone page (C7), which are not files the owner keeps by name.

## Testing

- **Unit:**
  - `file_time`: none, negative, future, millisecond rounding;
  - `place` setting the time it is given, and leaving it when given none;
  - an offer's body with its file's date, and none with the setting off.
- **Integration:** `server/tests/two_computers.rs` sends a file with an old date each way and reads the time back.
- **On the device (checklist §149):**
  - a file dated by `touch -d` on the phone, shared to the throwaway server, and its time here;
  - a file dated here (`set_modified` in a test helper), offered to the phone, and `stat -c %Y` over adb.

## As built

As designed, with the review's fixes. The rule is `core::protocol::file_time`. On the computer, `place` sets the date once the file is in place (`share::set_date`), and `offers::with_date` adds it to what goes out. On the phone, `FileDates` holds the rule, and `DownloadTarget.date` sets it.

## Device run (the OPPO CPH2359, Android 14, against a throwaway server built from this branch)

- **Computer to phone:** a file dated 2026-06-01 12:00 here, sent with `pocketdesk send`, landed in Downloads/Pocketdesk dated 2026-06-01 12:00 by `stat`, and MediaStore's `date_modified` said the same.
- **Phone to computer:** the same file shared from the phone's share screen landed here dated 2026-06-01 12:00.
- **Two computers** (`server/tests/two_computers.rs`): a file dated June each way kept its date.

## Review

A fresh review found 0 Critical, 1 Important and 6 Minor.

**Important, fixed:** a file downloaded from the remote file browser carried no date. It takes its listing's date now.

**Minor, fixed one commit each:**

- a share from a stream of unknown size, sent through its copy, carried none;
- where Android will not date the file by its path, MediaStore's date is set instead;
- `set_date` asked for write access, which a read-only file refuses; it asks only for the right to change times now, and a test covers a read-only file;
- the setting's off case is tested without the owner's config, and a single file's offer is tested;
- the docs: `config.toml.example` and `docs/protocol.md`.

**Left:** a test that the phone's share request carries the date. It is checked on the device instead.
