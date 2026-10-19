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
- **Windows:** `GetSystemTimes` sampled twice 250 ms apart (or against the last sample within a minute), `GlobalMemoryStatusEx`, `GetLogicalDriveStringsW` with `GetDriveTypeW`/`GetDiskFreeSpaceExW`, `GetTickCount64`.
- **Linux:** `/proc/stat`, `/proc/meminfo` (`MemTotal - MemAvailable`), `df -kP`, `/proc/uptime`.
- **The battery:** the battery plugin's own reading.

The parsers are pure and tested.

## The phone

- "This computer" in each computer's menu on Home opens a sheet: CPU, memory and each disk as a bar with its numbers, the battery, uptime. It asks again every 3 s while open.
- "Off" says the computer does not share its state.

## Tests

- **Computer:** the parsers (`/proc/stat`, `/proc/meminfo`, `df -kP`, `/proc/uptime`), the CPU arithmetic, the body's shape.
- **Phone:** JVM tests of the sizes' words and the bars.
- **Owed (device):** the sheet against this box and Linux.
