# Choose where received files land (C2)

Date: 2026-10-18. Plan: `docs/research/2026-10-17-next-cycle.md` §C2.

## The problem

Both ends put every received file in one place nobody chose: the computer in its Downloads folder (else the home folder), the phone in Downloads/Pocketdesk. "Add ability to change default save location" is a standing request against KDE Connect (KDE Discuss, August 2026). `POCKETDESK_DOWNLOAD_DIR` exists, but only as a test seam.

## What changes

### The computer

- **The setting.** Desktop Settings, "Files from phones land in" (`downloads_folder`, empty by default, meaning Downloads), with Choose folder… and Use Downloads.
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
