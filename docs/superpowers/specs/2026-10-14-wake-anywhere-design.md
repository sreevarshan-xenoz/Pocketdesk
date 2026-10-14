# Wake the computer from anywhere, without a second device (roadmap N6)

Date: 2026-10-14. Status: approved (autopilot; the user asked for "wake from anywhere without another always-on device", with everything customizable in Settings).

## Why

Wake is broken today. The phone's Wake button asks the saved computer to send the magic packet (`kdeconnect.pocketdesk.wol`), but that computer is the one asleep, so nothing answers. Wake only worked when a different, always-on machine ran the server at that address. The MAC is typed by hand under Advanced, the phone's own sender (`RustCore.sendWol`) is never called, "sent" means only that the request was written, and nothing says whether the computer can be woken at all.

Waking from anywhere without a second always-on device needs two things:
- at home, the phone sends the magic packet itself;
- away, it is sent to the home's public address, and the home router forwards it (a port forward the user sets up once).

## Design

### The desktop tells the phone how to wake it

- **Packet.** On every paired connection the server pushes `kdeconnect.pocketdesk.wake_info {mac, broadcast, wired, port}`:
  - `mac` and `broadcast` (the subnet's directed broadcast) belong to the network adapter the phone's control connection arrived on. A connection over a tunnel (Tailscale, adb reverse) has no adapter MAC, so the default route's adapter is used instead.
  - `wired` says whether that adapter is Ethernet. Most Wi-Fi adapters cannot wake a computer from shutdown or hibernation.
  - `port` is the outside wake port the user set on the desktop (`wake_port`, 0 = not set up).
- **Off switch.** `wake_announce` (on by default) stops the push.
- **Who sees it.** The packet goes only to paired phones, as the other pushes do. A MAC is an identifier, so it is never logged or put in a debug report.
- **Code.** `core::local::wake` finds the adapter with `netdev` (already in the graph through iroh): the adapter holding a given local IPv4 address, else the default one. It has no MAC for loopback or tunnels.

### The phone sends the magic packet itself

- **Storing it.** The phone saves the facts per computer (`HostStore`), learned while connected, as it does the WAN identity. A typed MAC still works and is kept.
- **Sending.** A new bridge call `wake(mac, targets)` sends the 102-byte packet to each target three times, 100 ms apart, from one UDP socket with broadcast on. `core::tailscale::wol` gains `magic_packet(mac)` (tested), which `send_wol` uses.
- **Choosing targets.** `WakePlan` (Kotlin, JVM-tested) picks them from the method, the learned facts and the user's settings:
  - **Auto (default):** the subnet broadcast and 255.255.255.255 on port 9, plus the outside address when one is known and a port is set.
  - **This network only:** the broadcasts only.
  - **Through another computer:** the old relay path, through a saved computer the user picks (an always-on one).
  - **Off:** no Wake button.
- **The outside address.** Defaults to the computer's last public IPv4, taken from the WAN identity the phone already caches (its `directAddrs`), with the port the desktop announced. The user can replace both, for example with a dynamic-DNS name.

### One tap: wake, then connect

- **Waking.** Wake on a computer's card sends the packet and shows "Waking…". The card's existing reachability probe then reports when the computer answers. After 90 s without an answer it says so ("No answer: check that wake is allowed on the computer").
- **Connect on a sleeping computer.** Connect on a card the phone cannot reach, for a computer it can wake, offers "Wake and connect". The phone wakes it, waits for it to answer (up to 90 s), and connects. Away from home it cannot probe, so it waits 30 s and then connects over the WAN path, which retries on its own.

### Settings

Everything is the user's to set:
- **Phone, per computer (the card's menu, "Wake settings"):**
  - the method (Auto, This network only, Through another computer, Off);
  - the relay computer;
  - the outside address and port;
  - the MAC, pre-filled with the learned one.
- **Desktop, Settings "Wake from the phone":**
  - the adapter, its MAC and whether it is wired, read-only;
  - whether the adapter allows magic-packet wake, where the system says;
  - "Tell paired phones how to wake this computer" (`wake_announce`);
  - "Outside wake port" (`wake_port`), with the router step spelled out: forward UDP on that port to this computer's address.

### Diagnostics

A new "Wake" row:
- **ok:** a wired adapter that allows magic-packet wake.
- **warn:** Wi-Fi (can usually wake only from sleep), or wake turned off on the adapter. The adapter's power settings or the firmware (BIOS/UEFI) can turn it off.
- **unknown:** the system does not say.

On Windows the check reads `Get-NetAdapterPowerManagement` (`WakeOnMagicPacket`); on Linux, `/sys/class/net/<if>/device/power/wakeup` and `ethtool`'s `Wake-on` when it is installed.

## Honest limits

- **The router.** A forward to the computer's address reaches it only while the router still knows the computer's hardware address (its ARP entry). That is minutes to hours after it sleeps, depending on the router. Routers that allow a forward to the subnet's broadcast, or a static ARP entry, work all the time. The desktop's Settings and the README say this.
- **Wi-Fi and full shutdown.** Wi-Fi rarely wakes from shutdown. Windows' fast startup can block wake from shutdown on some boards.
- **What cannot be tested here.** Sleeping this box is off limits. The device run checks that the phone's packets arrive on the desktop (a UDP listener on port 9 here), not that a sleeping computer wakes. That is owed to the user.

## Tests

- **core:**
  - `magic_packet` (layout, MAC parsing);
  - `wake::facts_for` against a fixed adapter list (the adapter holding an address, a tunnel falling back to the default, no MAC);
  - the config keys.
- **server:** the `wake_info` packet's body, and that it is not sent when `wake_announce` is off.
- **bridge:** `wake` sends to each target (a loopback receiver counts the packets and checks their bytes).
- **Kotlin:** `WakePlan` for each method, learned and typed MACs, the public-address pick (private, CGNAT and IPv6 addresses skipped), and port 0 meaning no outside target.
- **Desktop:** the Wake settings' validation (port 1–65535 or empty).

## Order of work

1. `core`: `magic_packet`, `core::local::wake`, config keys.
2. `server`: the `wake_info` push.
3. `bridge`: cache `wake_info`, plus the `wake` call and its JNI exports.
4. App: `HostStore` facts, `WakePlan`, the Wake button and "Wake and connect", and the per-computer Wake settings.
5. Desktop: Settings "Wake from the phone", and the Diagnostics row.
6. Docs: README, checklist §42, roadmap N6, AGENTS.

## As built

- **Followed the design,** with three deviations:
  - **Desktop Settings** names the adapter, whether it is wired, and its MAC. Whether the adapter allows wake is in Diagnostics only: the query can take seconds, too long for the Settings tab.
  - **The phone's MAC field** is left blank to mean "the one it announced", rather than pre-filled with it.
  - **Connect checks first.** Connect on a computer that can be woken probes its control port (1.5 s). If it does not answer, the phone offers "Wake and connect" or "Connect anyway". Settings "Offer to wake a computer that does not answer" (on by default) turns the check off.
- **Reading wake on Windows.** `Get-NetAdapterPowerManagement` fails on this box's Realtek driver ("A device attached to the system is not functioning"). NDIS's standard `*WakeOnMagicPacket` keyword answers for it, and does not depend on the display language. On Linux only `ethtool`'s `Wake-on:` counts. sysfs' `power/wakeup` does not say the wake is by magic packet, so without ethtool the answer is "unknown".
- **Fresh review (0 Critical, 4 Important, 9 Minor), fixed one commit each:**
  - **Wrong computer's MAC.** The bridge kept one wake-info slot whatever connection filled it, so a session could save another computer's MAC. The slot carries the host and port it came from, and is handed out only for that computer.
  - **Awake detection.** It matched mDNS addresses, so a computer saved by its tailnet address never counted as awake. The phone now probes the control port every 2 s.
  - **Old relay setups.** A legacy MAC with no new settings stays a relay to the saved computer itself, unless that computer announced the same MAC (then it is the sleeping one).
  - **"Wake and connect"** on Connect was missing (above).
  - **Minors fixed:**
    - away from home the MAC is no longer broadcast on someone else's network (a new "Away" outcome says when nothing can be sent);
    - learned targets must be IPv4 addresses;
    - the outside address takes no port, and IPv6 is bracketed;
    - a computer that stops announcing makes phones forget (an empty `wake_info`);
    - the relay no longer logs the MAC;
    - PowerShell's typographic quotes are escaped in adapter names;
    - the Linux sysfs fallback (above);
    - the docs.
  - **Accepted, not fixed:**
    - "at home" is judged by the phone sharing the computer's broadcast address, so a different network with the same numbering counts as home and gets the broadcast;
    - the outside address is whatever the user or the computer's last public address says. A paired computer can make each Wake tap send 3 packets per target to an address of its choosing, with no amplification.
- **Device run (A71 and this box), with a UDP listener standing in for the sleeping computer:**
  - the session taught the phone the MAC and broadcast;
  - Wake sent the 9 expected packets, and the card said the computer was awake;
  - the Wake settings dialog and the "does not answer" offer appeared as designed.

  A real wake, at home and from outside, is owed (checklist §42).
