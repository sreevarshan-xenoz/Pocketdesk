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
        /// Leave the firewall rules alone: the Windows installer adds them
        /// itself, already elevated, and runs this as the signed-in user
        #[arg(long, hide = true)]
        no_firewall: bool,
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
        /// paired computer, also by its name. Several, with commas between,
        /// each get it (--wait waits for each in turn)
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
    /// Have a phone dial a number (L4): its dialer opens with it, and you
    /// press Call there. Exit codes: 0 queued, 1 failed, 2 bad arguments, 3
    /// no server running
    Call {
        /// The phone: its device id, or 6 or more of its first characters
        phone: String,
        /// The number: digits, a + only at the start
        number: String,
        /// Print the local API's JSON (see docs/local-api.md)
        #[arg(long)]
        json: bool,
    },
    /// Ask the owner's phone a Yes or No question and wait for the answer
    /// (BF3): it comes as a notification on each linked phone with the Ask
    /// permission; No is one tap there, Yes takes the phone's fingerprint or
    /// screen lock. Prints yes or no. Exit codes: 0 yes, 1 no, 2 no answer
    /// in time (or bad arguments, or no paired phone with "Answer my
    /// questions" on, said at once), 3 the question could not be left for
    /// the server
    Ask {
        /// How long to wait for an answer, in seconds (1 to 3600)
        #[arg(long, default_value_t = pocketdesk_core::protocol::ask::DEFAULT_WAIT_SECS)]
        wait: u64,
        /// The question; its words are joined with spaces. Options go
        /// before it: after it, they are words of the question
        #[arg(required = true, trailing_var_arg = true)]
        question: Vec<String>,
    },
    /// Tell the phone when a command ends (BF1): run it here, its output
    /// passing through, or wait on a process already running (--pid); then
    /// the phones that show this computer's notifications get one saying
    /// how it ended, how long it took and its last lines. Exits as the
    /// command does (0 when watching a process that ended); 2 bad
    /// arguments
    Done {
        /// Wait on this process instead of running a command
        #[arg(long, conflicts_with = "command")]
        pid: Option<u32>,
        /// Leave the output's last lines out of the notification
        #[arg(long)]
        no_output: bool,
        /// Name the notification so, instead of by the command line (which
        /// can carry a secret, and may show on the phone's lock screen)
        #[arg(long)]
        title: Option<String>,
        /// The command and its arguments, after `--`
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },
    /// Send a command to a phone's automation apps (L6): Tasker, Automate or
    /// MacroDroid hear it as `dev.pocketdesk.android.event.COMMAND` with its
    /// name and text. Exit codes: 0 queued, 1 failed, 2 bad arguments, 3 no
    /// server running
    PhoneCommand {
        /// The phone: its device id, or 6 or more of its first characters
        phone: String,
        /// The command's name: 1 to 64 printable characters
        name: String,
        /// Its text, at most 4096 bytes
        #[arg(default_value = "")]
        text: String,
        /// Print the local API's JSON (see docs/local-api.md)
        #[arg(long)]
        json: bool,
    },
    /// The clipboard's history (D3): the texts that crossed between this
    /// computer and its phones, newest first; or, with --send, one of them
    /// put on a phone's clipboard again. Exit codes: 0 done, 1 failed, 2 bad
    /// arguments, 3 no server running (for --send)
    Clipboard {
        /// Send the entry at this place in the list (1 is the newest)
        #[arg(long, requires = "to", value_parser = clap::value_parser!(u32).range(1..))]
        send: Option<u32>,
        /// The phone to send it to: its device id, or 6 or more of its first characters
        #[arg(long, requires = "send")]
        to: Option<String>,
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
    /// Turn on a paired phone's Do Not Disturb for a while, or end it (D2):
    /// one with Mute on that keeps a link now ("Let the computer mute this
    /// phone" on in its Settings). Exit codes: 0 asked, 1 failed, 2 bad
    /// arguments, 3 no server running
    Quiet {
        /// The phone: its device id, or 6 or more of its first characters
        phone: String,
        /// For how many minutes, 1 to 1440 (a day)
        #[arg(
            long,
            default_value_t = 60,
            conflicts_with = "off",
            value_parser = clap::value_parser!(u32).range(1..=1440)
        )]
        minutes: u32,
        /// End the mute this computer asked for
        #[arg(long)]
        off: bool,
        /// Print the local API's JSON (see docs/local-api.md)
        #[arg(long)]
        json: bool,
    },
    /// Pause what the phones bring to this computer (K2a): their
    /// notifications and texts not shown, links not opened, a phone ringing
    /// this computer silent. With no flag, says whether it is paused. Exit codes: 0 done,
    /// 1 failed, 2 bad arguments
    Pause {
        /// For how many minutes, 1 to 10080 (a week)
        #[arg(
            long = "for",
            value_name = "MINUTES",
            conflicts_with_all = ["until_resumed", "resume"],
            value_parser = clap::value_parser!(u32).range(1..=10080)
        )]
        minutes: Option<u32>,
        /// Until resumed
        #[arg(long, conflicts_with = "resume")]
        until_resumed: bool,
        /// End the pause
        #[arg(long)]
        resume: bool,
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
    /// Have the phone dial the number a tel: or callto: link names (M8):
    /// what the link handler runs. With several phones that may, the
    /// desktop app asks which. Says how it went in a desktop notification.
    /// Exit codes: 0 queued or asked, 1 failed, 2 not a number, 3 no server
    Tel {
        /// The link, such as tel:+1-555-0100
        uri: String,
    },
    /// Open tel: and callto: links on the phone (M8), or stop with
    /// --remove. On Windows, pick Pocketdesk when Windows asks, or in
    /// Default apps
    TelHandler {
        /// Remove it
        #[arg(long)]
        remove: bool,
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
        /// Time-box the trust this PIN grants (e.g. 45s, 15m, 2h, 7d), for a
        /// guest's phone: it may send files and links only, and its sessions
        /// ask you first. Omit for permanent trust.
        #[arg(long, value_parser = parse_grant_secs)]
        grant: Option<u64>,
        /// The phone is someone else's: it pairs for good, may send files and
        /// links only, and its sessions ask you first (as a guest's)
        #[arg(long)]
        someone_else: bool,
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

    /// The Windows installer opens the firewall itself, already elevated,
    /// then registers the logon task as the user who ran it (BH1).
    #[cfg(windows)]
    #[test]
    fn install_can_leave_the_firewall_to_the_installer() {
        use clap::Parser;
        let parsed =
            super::Cli::try_parse_from(["pocketdesk", "install", "--no-firewall"]).unwrap();
        assert!(matches!(
            parsed.command,
            Some(super::Commands::Install {
                elevated: false,
                no_firewall: true
            })
        ));
        let parsed = super::Cli::try_parse_from(["pocketdesk", "install"]).unwrap();
        assert!(matches!(
            parsed.command,
            Some(super::Commands::Install {
                no_firewall: false,
                ..
            })
        ));
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
