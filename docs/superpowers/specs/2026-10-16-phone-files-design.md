# Browse the phone's storage from the computer (T10g)

Item 7 of `docs/research/2026-10-15-what-makes-it-worth-using.md`. Windows 11 puts the phone in File Explorer, KDE Connect browses it over SFTP (GSConnect #1610), and Pocketdesk only browses the other way.

## What the phone shares

Nothing, until its owner picks folders. Settings "Folders the computer may open" uses Android's folder picker (`ACTION_OPEN_DOCUMENT_TREE`): each pick is a persisted, read-only grant, listed with Remove. No storage permission is asked; the computer sees only those trees.

## Wire

- **Computer → phone, list:** `kdeconnect.pocketdesk.phone_files.list {root, path}`.
  - `root` is a granted tree's id, an index the phone gave.
  - `path` is a list of names under it; empty means the root.
- **Phone → computer, the listing:** `kdeconnect.pocketdesk.phone_files.listing {roots: [{id, name}], root, path, entries: [{name, dir, size, modified}], error?}`.
  - At most 2000 entries; hidden ones (a leading dot) are left out.
- **Computer → phone, fetch:** `kdeconnect.pocketdesk.phone_files.fetch {root, path}`.
  - The phone shares that file to its computer the way the share sheet does (F7a/T6a, resumable).
  - It lands in the computer's Downloads.

Both requests are held to the Files permission on the desktop. On the phone they are run only:
- when they come from its own computer;
- for a granted root;
- for names that are single path parts (no `/`, no `..`), looked up child by child under the tree.

## Desktop

- **Requests:** the desktop app's new "Phone" tab writes them to `<state>/phone_files_out/`, as the other outboxes are written, and the server sends them.
- **Listings:** the server keeps each phone's last listing in `<state>/phone_files.json` (owner-only), and the app shows it.
- **The Phone tab** has the roots as buttons, the folder's entries with Up, and Download for a file.

## Phone

The notification listener's tick takes the requests, as answers and background files do, and keeps the link while folders are shared.

## Tests

- **Core:** the packet bounds; the request outbox; the listing store.
- **Server:** listings kept per phone; a request needs a connected phone with Files on.
- **Kotlin (JVM):** `PhoneFilesRule`. It covers name checks, the own-computer check, roots by id, and hidden names.
- **Device (A71 → Windows):**
  - grant `Download`;
  - list it from the desktop;
  - open a sub-folder;
  - download a file, with the MD5s matching.
