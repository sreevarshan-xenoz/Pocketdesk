use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "pocketdesk", version)]
#[command(about = "Pocketdesk - your desktop from your phone, over LAN, Tailscale or iroh")]
pub struct Cli {
    #[arg(short, long)]
    pub verbose: bool,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Start the server daemon
    Start {
        /// Run detached: no console window on Windows, and logs go to daily
        /// files under the state directory instead of stderr. The logon task
        /// `install` registers starts the server this way.
        #[arg(long)]
        background: bool,
    },
    /// Register Pocketdesk to start at logon, start it now, and open its
    /// firewall ports (asks for administrator rights once, for the firewall)
    #[cfg(windows)]
    Install {
        /// Run the server elevated so input reaches elevated windows too
        #[arg(long)]
        elevated: bool,
    },
    /// Remove the logon task, the firewall rules and the notification identity
    #[cfg(windows)]
    Uninstall,
    /// Add the firewall rules (needs administrator rights; `install` runs it)
    #[cfg(windows)]
    #[command(hide = true)]
    FirewallAdd,
    /// Remove the firewall rules (needs administrator rights; `uninstall` runs it)
    #[cfg(windows)]
    #[command(hide = true)]
    FirewallRemove,
    /// Stop a running server process started with `pocketdesk start`
    Stop,
    /// Print local tailscale status
    Status {
        /// Print the local API's JSON instead: the server, the phones in a
        /// session, and the internet endpoint (see docs/local-api.md)
        #[arg(long)]
        json: bool,
    },
    /// List the paired phones: what each may do, whether a key is bound to
    /// it, and whether it is in a session now
    Devices {
        /// Print the local API's JSON (see docs/local-api.md)
        #[arg(long)]
        json: bool,
    },
    /// Send a file or a folder to a paired phone in a session (or a file to a
    /// paired computer whose link is up), as the desktop app's Files tab
    /// does. Exit codes: 0 done, 1 failed, 2 bad arguments, 3 no server
    /// running, 4 still waiting when --wait ran out
    Send {
        /// The phone: its device id, or 6 or more of its first characters; or a
        /// paired computer, also by its name
        phone: String,
        /// The file or folder to send
        path: std::path::PathBuf,
        /// Wait up to this many seconds for the phone to save it
        #[arg(long, value_name = "SECONDS")]
        wait: Option<u64>,
        /// Print the local API's JSON (see docs/local-api.md)
        #[arg(long)]
        json: bool,
    },
    /// Hand a web link to a paired phone in a session: it waits there as a
    /// notification that opens it. Exit codes: 0 sent, 1 failed, 2 bad
    /// arguments (not an http or https link), 3 no server running
    Link {
        /// The phone: its device id, or 6 or more of its first characters
        phone: String,
        /// The link (http or https)
        url: String,
        /// Print the local API's JSON (see docs/local-api.md)
        #[arg(long)]
        json: bool,
    },
    /// Ring a paired phone that keeps a link now (a session, or "Ring from
    /// the computer" on in its Settings), at most once a minute. Exit
    /// codes: 0 asked, 1 failed, 2 bad arguments, 3 no server running
    Ring {
        /// The phone: its device id, or 6 or more of its first characters
        phone: String,
        /// Print the local API's JSON (see docs/local-api.md)
        #[arg(long)]
        json: bool,
    },
    /// Send files or folders to your phone, the one connected on this
    /// network (in a session first): what the file manager's "Send to
    /// phone" runs. Says how it went in a desktop notification too.
    /// Exit codes: 0 queued, 1 failed, 2 nothing to send, 3 no server running
    SendToPhone {
        /// The files and folders
        paths: Vec<std::path::PathBuf>,
    },
    /// Add "Send to phone" to the file manager (Explorer's Send To; the
    /// Nautilus, Nemo and Dolphin menus), or remove it with --remove
    SendMenu {
        /// Remove it
        #[arg(long)]
        remove: bool,
    },
    /// List the files sent to phones, newest first, and how each went
    Sent {
        /// How many to list
        #[arg(short, long, default_value_t = 20)]
        count: usize,
        /// Print the local API's JSON (see docs/local-api.md)
        #[arg(long)]
        json: bool,
    },
    /// Print what Pocketdesk does as it happens, until stopped: sessions,
    /// phones, batteries, notifications, texts, files and links (the event log)
    Events {
        /// Print the events already in the log first
        #[arg(long)]
        since_start: bool,
        /// Print each event as the local API's JSON, one object per line
        /// (see docs/local-api.md)
        #[arg(long)]
        json: bool,
    },
    /// The host the Pocketdesk browser extension talks to (browsers start it
    /// themselves); --install registers it with the browsers found
    BrowserHost {
        /// Register it with the browsers found
        #[arg(long)]
        install: bool,
        /// Remove its registrations
        #[arg(long, conflicts_with = "install")]
        remove: bool,
    },
    /// List the files received from phones, newest first
    Received {
        /// How many to list
        #[arg(short, long, default_value_t = 20)]
        count: usize,
        /// Print the local API's JSON (see docs/local-api.md)
        #[arg(long)]
        json: bool,
    },
    /// Show recorded streaming-session outcomes (R4 A2 telemetry log)
    Sessions {
        /// Number of recent sessions to list
        #[arg(short, long, default_value_t = 10)]
        count: usize,
        /// Emit the retained per-session records as JSON lines (one object per
        /// session, including the p50/p90/p95/p99/max tails)
        #[arg(long)]
        json: bool,
    },
    /// Run the pinned encoder benchmark on this machine and, given a baseline,
    /// gate on it (roadmap 2195-2200). Exit codes: 0 pass, 1 regression,
    /// 2 bad arguments, 3 no comparable baseline, 4 could not measure.
    Bench {
        /// Encoder rung to measure. Deliberately not "auto": a baseline whose
        /// backend was decided by probe order cannot be compared after a driver
        /// update.
        #[arg(long, default_value = "software")]
        target: String,
        /// Frames to encode; the first 30 are discarded as warm-up
        #[arg(
            long,
            default_value_t = pocketdesk_core::streaming::bench::DEFAULT_FRAMES
        )]
        frames: u32,
        /// Repeat the clip and keep the fastest run: other work on the machine
        /// can only make an encode slower, so a busy desktop still records a
        /// usable baseline. 1 is fine on an idle CI runner.
        #[arg(long, default_value_t = 3)]
        repeat: u32,
        /// Emit the record as JSON on stdout
        #[arg(long)]
        json: bool,
        /// Write the record here. A file is written as given; a directory (or a
        /// path with no extension) gets one file named after backend + host
        #[arg(long, value_name = "PATH")]
        record: Option<std::path::PathBuf>,
        /// Baseline record to compare against: a file, or a directory of records
        /// matched by host
        #[arg(long, value_name = "PATH")]
        baseline: Option<std::path::PathBuf>,
        /// How much a percentile may grow before it counts as a regression.
        /// Records are whole milliseconds, so the allowance has to be wide
        /// enough that a one-millisecond rounding at the median is not a red
        /// build; measured run-to-run noise on this workload is <=1 ms.
        #[arg(long, default_value_t = 50)]
        tolerance_pct: u64,
    },
    /// List peers currently visible on the tailnet
    List {
        /// Print the local API's JSON (see docs/local-api.md)
        #[arg(long)]
        json: bool,
    },
    /// Continuously watch peer discovery events
    Watch {
        /// Poll interval in seconds
        #[arg(short, long, default_value_t = 10)]
        interval: u64,
    },
    /// Show negotiated protocol versions, transports, capture backends and
    /// codecs (generated from the constants the wire uses), plus the configured
    /// KDE Connect capability sets
    Capabilities {
        /// Emit the machine-readable JSON report
        #[arg(long)]
        json: bool,
        /// Emit the Markdown block that docs/capabilities.md is generated from
        #[arg(long)]
        markdown: bool,
        /// Emit the control channel's packet catalogue (docs/packets.md)
        #[arg(long, conflicts_with_all = ["json", "markdown"])]
        packets: bool,
    },
    /// Connect to a peer and perform a basic control-channel handshake
    Connect {
        /// Peer hostname, MagicDNS name, or Tailscale IP
        peer: String,
        /// Control port to connect to
        #[arg(short, long, default_value_t = pocketdesk_core::DEFAULT_CONTROL_PORT)]
        port: u16,
    },
    /// Set or generate a temporary pairing PIN
    Pair {
        /// Optional 6-digit PIN
        pin: Option<String>,
        /// Time-box the trust this PIN grants (e.g. 45s, 15m, 2h, 7d).
        /// Omit for permanent trust. R4 D3 one-off support sessions.
        #[arg(long, value_parser = parse_grant_secs)]
        grant: Option<u64>,
        /// Print the local API's JSON (see docs/local-api.md)
        #[arg(long)]
        json: bool,
    },
    /// Remove a paired device from the trust store (all if no id is given)
    Unpair {
        /// Device id to untrust (omit to untrust all devices)
        device_id: Option<String>,
        /// Print the local API's JSON (see docs/local-api.md)
        #[arg(long)]
        json: bool,
    },
    /// Drop a live streaming session (R4 D2): device id, a unique id
    /// prefix of >=6 chars, the peer IP, or "all"
    Kick {
        /// Session target — see the subcommand help
        device: String,
        /// Print the local API's JSON (see docs/local-api.md). Exit codes:
        /// 0 asked, 1 failed, 2 nothing named, 3 no server running
        #[arg(long)]
        json: bool,
    },
    /// Run your own relay for phones away from home (instead of n0's public
    /// ones). It must be reachable from the internet; set its URL as
    /// `relay_urls` in the desktop's config.toml
    Relay {
        /// Plain HTTP port (ignored with --hostname, which serves 80 and 443)
        #[arg(long, default_value_t = 3340)]
        port: u16,
        /// Serve HTTPS for this name, with a Let's Encrypt certificate
        #[arg(long)]
        hostname: Option<String>,
        /// Contact address for Let's Encrypt
        #[arg(long, requires = "hostname")]
        email: Option<String>,
        /// Admit this endpoint id (repeatable; see `pocketdesk relay-ids`)
        #[arg(long)]
        allow: Vec<String>,
        /// Admit this desktop and its paired phones (when the relay runs here)
        #[arg(long)]
        allow_paired: bool,
        /// Admit anyone (strangers can use its bandwidth)
        #[arg(long)]
        open: bool,
    },
    /// Print the endpoint ids a relay elsewhere should --allow: this
    /// computer's, and each paired phone's
    RelayIds {
        /// Print the local API's JSON (see docs/local-api.md)
        #[arg(long)]
        json: bool,
    },
}

/// Parse a `--grant` duration (`45s`, `15m`, `2h`, `7d`, combinable like
/// `1h30m`) into seconds. Used as a clap `value_parser` so bad input fails
/// at argument parsing.
pub fn parse_grant_secs(input: &str) -> Result<u64, String> {
    let unit_secs = |c: char| match c {
        's' => Some(1u64),
        'm' => Some(60),
        'h' => Some(3600),
        'd' => Some(86400),
        _ => None,
    };
    let mut total = 0u64;
    let mut digits = String::new();
    let mut saw_unit = false;
    for c in input.trim().chars() {
        if c.is_ascii_digit() {
            digits.push(c);
        } else {
            let unit = unit_secs(c)
                .ok_or_else(|| format!("invalid unit '{c}' in '{input}' (expected s/m/h/d)"))?;
            if digits.is_empty() {
                return Err(format!("missing number before '{c}' in '{input}'"));
            }
            total += digits.parse::<u64>().map_err(|e| e.to_string())? * unit;
            digits.clear();
            saw_unit = true;
        }
    }
    if !digits.is_empty() || !saw_unit {
        return Err(format!(
            "'{input}' must be a number with a unit, e.g. 15m (s/m/h/d, combinable: 1h30m)"
        ));
    }
    if total == 0 || total > 365 * 86400 {
        return Err(format!("grant must be between 1s and 365d, got '{input}'"));
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::parse_grant_secs;

    /// `scripts/install.sh` reads the installed version this way to decide
    /// whether an upgrade is due.
    #[test]
    fn the_version_flag_prints_the_crate_version() {
        use clap::Parser;
        let err = super::Cli::try_parse_from(["pocketdesk", "--version"]).unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::DisplayVersion);
        assert!(err.to_string().contains(env!("CARGO_PKG_VERSION")), "{err}");
    }

    #[test]
    fn grant_durations_parse() {
        assert_eq!(parse_grant_secs("45s"), Ok(45));
        assert_eq!(parse_grant_secs("15m"), Ok(900));
        assert_eq!(parse_grant_secs("2h"), Ok(7200));
        assert_eq!(parse_grant_secs("1h30m"), Ok(5400));
        assert_eq!(parse_grant_secs(" 10m "), Ok(600));
    }

    #[test]
    fn grant_rejects_bad_input() {
        for bad in ["", "15", "15x", "m", "1h-30m", "0s", "366d", "1w"] {
            assert!(parse_grant_secs(bad).is_err(), "must reject {bad:?}");
        }
    }
}
