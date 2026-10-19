# F2: the computer's state on the phone (2026-10-20)

Plan: `docs/research/2026-10-20-next-cycle.md`.

## What

The phone shows how its computer is doing, without a session:
- CPU in use;
- memory used of total;
- each fixed disk's space used of total;
- the battery (when it has one);
- how long it has been up.

## The wire

`kdeconnect.pocketdesk.system`, phone → computer, answered with:

```json
{
  "cpu": 23.5,
  "memory": {"used": 8589934592, "total": 17179869184},
  "disks": [{"name": "C:", "used": 300000000000, "total": 500000000000}],
  "battery": {"charge": 80, "charging": true},
  "uptime_secs": 86400
}
```

- `cpu` is a percentage, null when it cannot be read.
- `battery` is null on a computer without one.
- Sizes are bytes.
- **Disks are the fixed ones:**
  - Windows: drive letters whose type is fixed;
  - Linux: `df -kP`'s `/dev/` filesystems, by mount point.
- **Settings "Share this computer's state"** (desktop, Phones; `share_state`, on by default). Off, the answer is `{"off": true}`.
- **No new permission:** a paired phone already sees the battery. Unpaired phones never reach plugins.

## The computer

`server/src/system_state.rs` reads, on a blocking thread:
- **The CPU, on both:** two samples 250 ms apart, or against the last sample when it is between 200 ms and a minute old (the sheet asks every 3 s).
- **Windows:** `GetSystemTimes`, `GlobalMemoryStatusEx`, `GetLogicalDrives` with `GetDriveTypeW`/`GetDiskFreeSpaceExW`, `GetTickCount64`.
- **Linux:** `/proc/stat` (its first eight columns), `/proc/meminfo` (`MemTotal - MemAvailable`), `df -klP` within 3 s, `/proc/uptime`.
- **The battery:** the battery plugin's own reading.

The parsers are pure and tested.

## The phone

- "This computer" in each computer's menu on Home opens a sheet: CPU, memory and each disk as a bar with its numbers, the battery, uptime. It asks again every 3 s while open.
- "Off" says the computer does not share its state.

## Tests

- **Computer:** the parsers (`/proc/stat`, `/proc/meminfo`, `df -kP`, `/proc/uptime`), the CPU arithmetic, the body's shape.
- **Phone:** JVM tests of the sizes' words and the bars.
- **Owed (device):** the sheet against this box and Linux.

## As built

As specified, with the details above:
- **The menu item:** "How it is doing" on the phone.
- **The disks:** each device once, at its shortest mount point. Optical drives, zram and what is under `/media` or `/run/media` are left out.
- **When the computer goes quiet:** the last answer stays, marked "Not answering now: this is how it was".
- **The asking:** only while the app is in front.
- **Not checked on a device** (the owner's night of features): checklist §127.

## Review

A fresh review found 0 Critical and 4 Important issues, fixed one commit each:
- `df` with no deadline (a dead network mount hung it): now `df -klP` within 3 s;
- a disk once per mount, and USB sticks counted as fixed;
- the sheet asking every 3 s in the background;
- old readings shown as live.

Minors fixed:
- guest time counted twice;
- a CPU reading that is not a number;
- SECURITY.md limit 33;
- the spec's calls.

Minor left: two phones asking at once share the CPU sample, so each reads a shorter window.
