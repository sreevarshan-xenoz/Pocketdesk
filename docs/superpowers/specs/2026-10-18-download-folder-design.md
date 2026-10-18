# Choose where received files land (C2)

Date: 2026-10-18. Plan: `docs/research/2026-10-17-next-cycle.md` §C2.

## The problem

Both ends put every received file in one place nobody chose: the computer in its Downloads folder (else the home folder), the phone in Downloads/Pocketdesk. "Add ability to change default save location" is a standing request against KDE Connect (KDE Discuss, August 2026). `POCKETDESK_DOWNLOAD_DIR` exists, but only as a test seam.

## What changes

### The computer

- **The setting.** Desktop Settings, "Received files land in" (`downloads_folder`, empty by default, meaning Downloads), with Choose folder… and Use Downloads.
- **Where it applies.** It covers every file the server receives: phones' shares and folder sends, a computer's files (2h), and a KDE Connect device's (3c).
- **When it applies.** It is read at each file, so a change applies at once.
- **A folder that is gone** (or not absolute, or not a folder): the file lands in Downloads, the log says why, and the received notice names where it went, as it already does ("… is in Downloads.").
- The test seam keeps precedence.

### The phone

- **The setting.** "Files from computers go to" in its Settings shows where they go now:
  - Downloads/Pocketdesk, the default;
  - or a folder picked with Android's folder picker, with a lasting write grant.
- **How a file is written there.** Into a picked folder, a file goes through the Storage Access Framework:
  - a document named `<name>.part` while it arrives, renamed to its name once whole;
  - a folder send's files under a new folder of its name, its sub-folders found or made;
  - a part kept for a retry is continued in place, as in MediaStore.
- **A folder whose grant was taken back, or that is gone:** the file goes to Downloads/Pocketdesk, and the notification says so.
- Opening a finished file from the notification works for either place.

## Tests

- **Computer:** the folder is used when it is a folder, Downloads otherwise; the seam first; the Settings row in its section; the config key held to the Settings table.
- **Phone (JVM):** the choice between the picked folder and Downloads (`DownloadPlace`), and the part's name and the finished one.
- **Device (A71 and this box):**
  - a file each way into a chosen folder, then into Downloads again;
  - the phone's folder grant taken back, then a file;
  - a folder send into a picked folder;
  - a broken download continued in a picked folder.

## As built

As above, with these additions:
- **The computer:**
  - Pocketdesk's own state and config folders, and anything inside them, are refused as the folder.
  - Settings that cannot be read keep the last folder read.
  - Each warning is logged once, not per file.
- **The phone:**
  - A name the picked folder already holds gets the next free one (`photo (1).jpg`) before the part is made, so the rename finds its name free.
  - A provider that cannot rename has the file written under its own name at once.
  - A picked folder on another volume is not refused for the phone's own free space.
  - A picked folder that fails midway falls back to Downloads/Pocketdesk and leaves no part behind.
  - Picking the same folder again keeps its grant, and a folder the computer may also open keeps its read grant.
- **Both apps' words** no longer say Downloads where the folder may be another.

## The device run

On the A71 against a throwaway server on this box, the phone in the background with files on:
- a folder picked in Settings with Android's picker (Documents/PDtest, granted);
- a file from the computer saved there (checksum matching);
- the same file again saved as `c2photo (1).jpg`;
- a folder with a sub-folder arriving whole under a new folder of its name;
- the picked folder deleted, then a file: saved in Downloads/Pocketdesk, with the notification saying why.

**Owed (checklist §105):**
- the computer's folder end to end: a phone's share into a chosen folder could not be started from adb (the shell cannot grant a file), and the screenshot route did not start;
- a grant taken back in Android's settings;
- a download broken and continued in a picked folder;
- an SD card or cloud folder.

## Review

**Fresh review (0 Critical, 8 Important), fixed one commit each:**
- a second file of one name left a part;
- picking the same folder revoked its grant;
- the free space checked on the wrong volume;
- documents told from MediaStore rows by a provider lookup package visibility can hide;
- a bad folder aborting instead of falling back;
- an unreadable config sending files to Downloads unseen;
- the server's own folders allowed;
- words and docs that said Downloads.

**Minors fixed:**
- warnings once;
- a root folder's notice;
- the fused doc comment.

**Left as is:**
- **Blocking checks:** the folder's existence check runs in the packet path, which a dead network share could stall.
- **A vanished folder:** one that goes between that check and the write is made again.
- **Old parts:** an old folder's parts after the folder changes.
- **Names a provider changes:** a folder send whose names the provider sanitizes.
- **Lookups on the main thread:** Settings reads the folder's name there.

