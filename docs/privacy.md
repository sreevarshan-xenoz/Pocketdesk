# Privacy policy

Pocketdesk is software you run on your own computer and phone. There is no Pocketdesk account and no Pocketdesk server, and the project collects nothing: no analytics, no crash reports, no update checks. What you send between your devices (the screen, input, sound, the clipboard, files, notifications and texts) goes from one of your devices to another, encrypted end to end with TLS 1.3 or QUIC, and is never readable by anyone else on the way.

To reach your computer from outside your home network, and to find it again after its address changes, Pocketdesk uses some public services by default. This page lists each one, what it sees, and how to turn it off. [docs/protocol.md](protocol.md) §4 has the technical detail, down to the source files.

## What leaves your devices by default

| Service | Who runs it | What it sees | How to turn it off |
|---|---|---|---|
| **Relays**, used to reach the computer from outside your network and as a fallback path: `use1-1`, `usw1-1`, `euc1-1` and `aps1-1.relay.n0.iroh.link` | n0, the company that builds iroh ([terms and privacy](https://iroh.computer/legal)) | Both ends' public IP addresses and their Pocketdesk WAN ids, which ids talk to each other, when, and how much. Never the content, which is encrypted end to end. | Desktop Settings → "Reachable from the internet" off (`internet_path = false`), or run your own relay (`pocketdesk relay`) and list it in `relay_urls`, so only yours is used. |
| **The Mainline DHT**, a public peer-to-peer directory, so a phone away from home finds the computer after its relay or address changes. It is joined through `router.bittorrent.com`, `dht.transmissionbt.com`, `dht.libtorrent.org` and `relay.pkarr.org` | Its volunteer nodes | A signed record under the computer's WAN id (by default only relay addresses, never private ones), and the public IP it was published from. When the phone looks it up from away: the phone's IP and the id it looks for. | Desktop Settings → "Found by id (DHT)" set to off (`dht_publish = "off"`), or "Reachable from the internet" off. |
| **Your home router**, asked to open a port for the direct path (UPnP, NAT-PMP or PCP) | You | A port-mapping request | "Reachable from the internet" off. |
| **Your DNS resolver**, to look up the relay and DHT host names | Your network's resolver. Only when the system lists no usable nameserver does the computer ask public resolvers (such as Cloudflare's 1.1.1.1) | Those host names | "Reachable from the internet" off. |

On your own network, the computer announces itself over mDNS so phones there list it. That stays on the local network. Desktop Settings → "Visible on this network" off (`lan_discovery = false`) stops it.

## Only when you turn them on

- **Home Assistant** (`home_assistant = true`): the computer's name, its paired phones by short id, their battery and connection state, and the kind of each event (never its content) go to the MQTT broker you name.
- **Your own relay with a host name** (`pocketdesk relay --hostname`): Let's Encrypt is asked for its certificate, with the e-mail address you give.
- **Tailscale**: when it runs, Pocketdesk asks the local Tailscale program for its status. Nothing is sent to Tailscale's servers by Pocketdesk.
- **The Windows installer's FFmpeg option**: when ticked and FFmpeg is missing, it installs FFmpeg through winget, Microsoft's package manager, which downloads it from its publisher.
- **The phone's background-limits link**: tapping it opens `dontkillmyapp.com` in your browser.

## The phone app

The Android app has no Google Play services, Firebase, ML Kit, analytics or advertising code. It talks to your paired computers, and, when connecting from away, to the same relays and DHT as above. Each phone's permissions on the computer (control, clipboard, files, notifications, texts and the rest) are set per phone in the desktop app's Devices page, and Android asks before the app may read notifications, texts, contacts or photos; each of those features also has its own switch in the phone's Settings.

## What stays on your devices

Pairings, keys, settings, the session history, the activity timeline, the clipboard history and the logs are files on your own computer and phone (`docs/protocol.md` §5). Nothing copies them anywhere. Uninstalling leaves them unless you ask for them to be deleted; the Windows uninstaller asks.

## Questions

Open an issue or a discussion on [GitHub](https://github.com/sreevarshan-xenoz/Pocketdesk). Report security problems privately, as [SECURITY.md](../SECURITY.md) describes.
