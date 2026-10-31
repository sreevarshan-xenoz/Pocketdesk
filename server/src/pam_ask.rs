//! `pocketdesk-pam-ask`: approve sudo, polkit and (with its own switch) the
//! lock screen on the owner's phone. PAM runs it through `pam_exec` as
//! `auth sufficient`, so a failure here only means the password prompt
//! comes next: the password always stays. Pocketdesk never edits
//! `/etc/pam.d`; Settings shows the lines to add.
//!
//! In PAM it:
//! 1. refuses a service it does not know, a lock screen unless "Lock screen
//!    too" is on, and (with "Only from this desk", on by default) a request
//!    from an ssh session;
//! 2. reads the phones' keys root pinned in `/etc/pocketdesk/pam/<user>.json`
//!    (owned by root and writable by root alone, as are its folders), with
//!    the computer's certificate hash; no pin means it gives up at once;
//! 3. becomes the user (it runs as root under sudo and polkit), reads the
//!    user's settings and gives up at once when the feature is off or the
//!    server is not running;
//! 4. makes a request with a fresh 32-byte nonce, bound to the computer, the
//!    service, the user, the command and the time, and leaves it for the
//!    server, which puts it to the linked phones or says at once that none
//!    is linked;
//! 5. waits at most the timeout (20 s by default, 5 to 60) for an answer,
//!    and approves only a Yes signed over this very request by the key root
//!    pinned for the phone that answered ([`crate::phone_approves::verify`]).
//!
//! Nothing the user's files or the server say can approve a request: they
//! can only carry the phone's signature, or fail to.
//!
//! Also `--pin USER` (as root: pin the keys the user's phones gave),
//! `--unpin USER`, `--lines` (the PAM lines) and `--test` (as the user: a dry
//! ask, from Settings' Test button, that touches no PAM).

use std::ffi::CString;
use std::io::Write as _;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use pocketdesk_core::local::approve::{self as files, PhoneKey, Request, Status};
use pocketdesk_core::local::config::Config;
use pocketdesk_core::protocol::approve::{
    Approval, HELPER_PATH, Kind, MAX_TIMEOUT_SECS, MIN_TIMEOUT_SECS, NONCE_LEN, PAM_SERVICES,
    PIN_DIR, clean_command, is_plain_name, pam_line,
};
use serde::{Deserialize, Serialize};

use crate::phone_approves::{Refusal, verify};

/// How long the server has to take a request before the helper gives up.
const TAKE_WAIT: Duration = Duration::from_secs(2);
const POLL: Duration = Duration::from_millis(100);

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
    UnknownService(String),
    LockScreenOff,
    Remote,
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
            GiveUp::NotPam => "not run by PAM".to_string(),
            GiveUp::UnknownService(s) => format!("PAM service {s:?} is not one it approves"),
            GiveUp::LockScreenOff => "the lock screen is not approved by phone here".to_string(),
            GiveUp::Remote => "only from this desk: a request from ssh is refused".to_string(),
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

/// One process between the asking program and the first process.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Ancestor {
    /// Its name (`/proc/<pid>/comm`).
    pub comm: String,
    /// Its environment names an ssh connection.
    pub ssh_env: bool,
    /// It is in a logind session marked remote.
    pub remote_session: bool,
}

/// Whether a request comes from away from this desk: a remote host PAM
/// was told of, or an ssh server, an ssh environment or a remote session
/// among the asking program's ancestors. A guard against prompts from ssh,
/// not a boundary: code running as the user could get past it, and still
/// needs the phone.
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

/// The checks made before anything is asked, each of which gives up at once.
pub fn precheck(
    kind: Kind,
    remote: bool,
    config: &Config,
    server_running: bool,
    pin: &Pin,
) -> Result<Vec<PhoneKey>, GiveUp> {
    if !config.phone_approves {
        return Err(GiveUp::Off);
    }
    if kind == Kind::Unlock && !config.phone_approves_lock_screen {
        return Err(GiveUp::LockScreenOff);
    }
    if remote && config.phone_approves_local_only {
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

/// Leave `approval` for the server and wait for a verified Yes, giving up
/// at once on "no phone" or a server that does not take it, and at the
/// timeout otherwise. Whatever happens, the request is taken back.
pub fn ask_and_wait(
    state_dir: &Path,
    approval: &Approval,
    phones: &[PhoneKey],
    sleep: impl Fn(Duration),
) -> Result<String, GiveUp> {
    let request = Request {
        approval: approval.clone(),
        phones: phones.iter().map(|k| k.device_id.clone()).collect(),
    };
    files::request(state_dir, &request)
        .map_err(|e| GiveUp::Broken(format!("the request could not be left: {e}")))?;
    let start = Instant::now();
    let wait = Duration::from_secs(approval.expires_unix.saturating_sub(approval.issued_unix));
    let result = loop {
        match files::status(state_dir, &approval.id) {
            Some(Status::NoPhone) => break Err(GiveUp::NoPhone),
            Some(Status::Answered {
                device_id,
                answer,
                signature,
            }) => {
                break verify(
                    approval,
                    &device_id,
                    answer,
                    signature.as_deref(),
                    phones,
                    now_unix(),
                )
                .map(|()| device_id)
                .map_err(GiveUp::Refused);
            }
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

/// The asking program's command line (sudo's: what it is about to run).
fn parent_command() -> String {
    // SAFETY: getppid has no preconditions.
    let ppid = unsafe { libc::getppid() } as u32;
    proc_read(ppid, "cmdline")
        .map(|raw| {
            let words: Vec<String> = raw
                .split(|b| *b == 0)
                .filter(|w| !w.is_empty())
                .map(|w| String::from_utf8_lossy(w).into_owned())
                .collect();
            clean_command(&words.join(" "))
        })
        .unwrap_or_default()
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

fn pam_mode(args: &[String]) -> Result<String, GiveUp> {
    let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    if var("PAM_TYPE").as_deref() != Some("auth") {
        return Err(GiveUp::NotPam);
    }
    let user = var("PAM_USER").ok_or(GiveUp::NoUser)?;
    let service = var("PAM_SERVICE").unwrap_or_default();
    if !is_plain_name(&user) {
        return Err(GiveUp::NoUser);
    }
    if !is_plain_name(&service) {
        return Err(GiveUp::UnknownService(service));
    }
    let forced = match args.iter().position(|a| a == "--kind") {
        Some(i) => match args.get(i + 1).map(String::as_str) {
            Some("admin") => Some(Kind::Admin),
            Some("unlock") => Some(Kind::Unlock),
            _ => return Err(GiveUp::Broken("--kind is admin or unlock".to_string())),
        },
        None => None,
    };
    let kind = kind_of(&service, forced).ok_or_else(|| GiveUp::UnknownService(service.clone()))?;
    // Read as root, before becoming the user: root's pin, and the asking
    // program, which the user's processes cannot change.
    let pin = read_pin(&user)?;
    let remote = is_remote(var("PAM_RHOST").as_deref(), &hostname(), &ancestors());
    let command = if kind == Kind::Admin && service != "polkit-1" {
        parent_command()
    } else {
        String::new()
    };
    let account = lookup_user(&user).ok_or(GiveUp::NoUser)?;
    become_user(&account)?;
    let state_dir = account.home.join(".local/state/pocketdesk");
    let config = Config::load_from(&account.home.join(".config/pocketdesk/config.toml"))
        .map_err(|e| GiveUp::Broken(format!("the settings do not read: {e}")))?;
    let running = pocketdesk_core::local::server::is_running(&state_dir);
    let phones = precheck(kind, remote, &config, running, &pin)?;
    let now = now_unix();
    let approval = Approval {
        id: uuid::Uuid::new_v4().to_string(),
        kind,
        service,
        user,
        command,
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

/// `--test`: a dry ask as the user, from Settings. Checks against root's
/// pin when there is one, else against the keys the phones gave (and says
/// so). Exit 0 approved, 1 refused or no answer, 2 not set up.
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
    let computer = match pocketdesk_core::local::identity::cert_fingerprint(&state_dir) {
        Ok(fp) => pocketdesk_core::local::identity::fingerprint_hex(fp),
        Err(_) => {
            eprintln!("This computer has no certificate yet: start Pocketdesk once.");
            return 2;
        }
    };
    let (pin, pinned) = match read_pin(&user) {
        Ok(pin) => (pin, true),
        Err(_) => (
            Pin {
                user: user.clone(),
                computer: computer.clone(),
                keys: files::keys(&state_dir),
                pinned_unix: 0,
            },
            false,
        ),
    };
    if pinned && pin.computer != computer {
        println!(
            "The pin is for another certificate: pin again (sudo {HELPER_PATH} --pin {user})."
        );
        return 2;
    }
    let running = pocketdesk_core::local::server::is_running(&state_dir);
    let mut config = config;
    // A test is about the phone, not the switches for ssh and lock screens.
    config.phone_approves_local_only = false;
    let phones = match precheck(Kind::Admin, false, &config, running, &pin) {
        Ok(p) => p,
        Err(e) => {
            println!("Not asked: {}.", e.said());
            return 2;
        }
    };
    let now = now_unix();
    let approval = Approval {
        id: uuid::Uuid::new_v4().to_string(),
        kind: Kind::Admin,
        service: "pocketdesk-test".to_string(),
        user: if is_plain_name(&user) {
            user
        } else {
            "you".to_string()
        },
        command: "A test from Settings: nothing runs".to_string(),
        computer,
        nonce: match new_nonce() {
            Ok(n) => n,
            Err(e) => {
                println!("{}", e.said());
                return 1;
            }
        },
        issued_unix: now,
        expires_unix: now + timeout_secs(&config),
    };
    println!(
        "Asked your phone; waiting up to {} s…",
        timeout_secs(&config)
    );
    match ask_and_wait(&state_dir, &approval, &phones, std::thread::sleep) {
        Ok(_) if pinned => {
            println!("Approved: the signature checks against the pinned key.");
            0
        }
        Ok(_) => {
            println!(
                "Approved by the phone's key, but it is not pinned for sudo yet: run sudo {HELPER_PATH} --pin <you>."
            );
            0
        }
        Err(e) => {
            println!("Not approved: {}.", e.said());
            1
        }
    }
}

/// Read a file the user owns, as root, refusing a link or someone else's.
fn read_users_file(path: &Path, uid: u32) -> Result<Vec<u8>, String> {
    let meta = std::fs::symlink_metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if !meta.file_type().is_file() || meta.uid() != uid {
        return Err(format!("{} is not the user's own file", path.display()));
    }
    std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))
}

/// `--pin USER` as root: pin the keys USER's phones gave, and this
/// computer's certificate hash.
fn pin_mode(user: Option<&String>) -> i32 {
    let Some(user) = user.filter(|u| is_plain_name(u)) else {
        eprintln!("Usage: sudo {HELPER_PATH} --pin USER");
        return 2;
    };
    // SAFETY: geteuid has no preconditions.
    if unsafe { libc::geteuid() } != 0 {
        eprintln!("Pinning needs root: sudo {HELPER_PATH} --pin {user}");
        return 2;
    }
    let Some(account) = lookup_user(user) else {
        eprintln!("No user {user}.");
        return 2;
    };
    let state = account.home.join(".local/state/pocketdesk");
    let keys = match read_users_file(
        &state.join(pocketdesk_core::local::paths::APPROVE_KEYS_FILE),
        account.uid,
    ) {
        Ok(raw) => {
            #[derive(Deserialize)]
            struct KeysFile {
                keys: Vec<PhoneKey>,
            }
            serde_json::from_slice::<KeysFile>(&raw)
                .map(|f| f.keys)
                .unwrap_or_default()
        }
        Err(e) => {
            eprintln!(
                "No phone has given a key yet ({e}). Turn on \"Approve my computer's requests\" on the phone."
            );
            return 1;
        }
    };
    let keys: Vec<PhoneKey> = keys
        .into_iter()
        .filter(|k| crate::phone_approves::p256_point(&k.public_key).is_some())
        .map(|mut k| {
            // Root's own reckoning, not the file's.
            k.fingerprint =
                crate::phone_approves::key_fingerprint(&k.public_key).unwrap_or_default();
            k
        })
        .collect();
    if keys.is_empty() {
        eprintln!("No phone has given a key yet.");
        return 1;
    }
    let cert = match read_users_file(
        &state.join(pocketdesk_core::local::paths::CERT_FILE),
        account.uid,
    ) {
        Ok(der) => ring::digest::digest(&ring::digest::SHA256, &der)
            .as_ref()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>(),
        Err(e) => {
            eprintln!("This computer has no certificate yet ({e}).");
            return 1;
        }
    };
    let pin = Pin {
        user: user.clone(),
        computer: cert,
        keys,
        pinned_unix: now_unix(),
    };
    let write = || -> std::io::Result<()> {
        use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _, PermissionsExt as _};
        for dir in ["/etc/pocketdesk", PIN_DIR] {
            match std::fs::DirBuilder::new().mode(0o755).create(dir) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e),
            }
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755))?;
        }
        let path = pin_path(user);
        let tmp = path.with_extension("json.tmp");
        let _ = std::fs::remove_file(&tmp);
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o644)
            .open(&tmp)?;
        file.write_all(serde_json::to_string_pretty(&pin)?.as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&tmp, &path)
    };
    if let Err(e) = write() {
        eprintln!("The pin could not be written: {e}");
        return 1;
    }
    println!("Pinned for {user} in {}:", pin_path(user).display());
    for k in &pin.keys {
        println!(
            "  phone {}: key {}{}",
            k.device_id,
            k.fingerprint,
            if k.biometric_only {
                " (fingerprint only)"
            } else {
                ""
            }
        );
    }
    println!("Check that each key matches the one the phone's Settings shows.");
    0
}

fn unpin_mode(user: Option<&String>) -> i32 {
    let Some(user) = user.filter(|u| is_plain_name(u)) else {
        eprintln!("Usage: sudo {HELPER_PATH} --unpin USER");
        return 2;
    };
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
         \n  --lines        the PAM lines to add yourself\n\
         \x20 --test         a dry ask, as you\n\
         \x20 --pin USER     as root: pin the keys USER's phones gave\n\
         \x20 --unpin USER   as root: forget them"
    );
}

/// The helper's `main`: its exit status.
pub fn main() -> i32 {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--lines") => {
            for (service, kind) in PAM_SERVICES {
                println!(
                    "# /etc/pam.d/{service} ({}): first of its auth lines",
                    kind.word()
                );
                println!("{}", pam_line());
            }
            0
        }
        Some("--test") => test_mode(),
        Some("--pin") => pin_mode(args.get(1)),
        Some("--unpin") => unpin_mode(args.get(1)),
        Some("--help" | "-h") => {
            usage();
            0
        }
        _ if std::env::var_os("PAM_TYPE").is_some() => match pam_mode(&args) {
            Ok(_) => 0,
            Err(e) => {
                // Said on stderr only: pam_exec drops it; the journal
                // keeps pam_exec's own failure line.
                eprintln!("pocketdesk-pam-ask: {}", e.said());
                if matches!(e, GiveUp::NoAnswer | GiveUp::Refused(_)) {
                    say("Not approved on the phone.");
                }
                1
            }
        },
        _ => {
            usage();
            2
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::phone_approves::tests::{TestPhone, approval};
    use pocketdesk_core::protocol::ask::Answer;

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

    #[test]
    fn it_is_off_by_default_and_gives_up_at_once_without_a_phone() {
        let phone = TestPhone::new("phone-a");
        let pinned = pin(&[&phone]);
        assert_eq!(
            precheck(Kind::Admin, false, &Config::default(), true, &pinned),
            Err(GiveUp::Off)
        );
        assert_eq!(
            precheck(Kind::Admin, false, &on(), false, &pinned),
            Err(GiveUp::ServerDown)
        );
        assert!(matches!(
            precheck(Kind::Admin, false, &on(), true, &pin(&[])),
            Err(GiveUp::NotPinned(_))
        ));
        let mut only_b = on();
        only_b.phone_approves_phones = vec!["phone-b".to_string()];
        assert!(matches!(
            precheck(Kind::Admin, false, &only_b, true, &pinned),
            Err(GiveUp::NotPinned(_))
        ));
        assert_eq!(
            precheck(Kind::Admin, false, &on(), true, &pinned),
            Ok(vec![phone.key()])
        );
    }

    #[test]
    fn no_linked_phone_gives_up_without_waiting() {
        // The server says no phone: the helper is done in well under the
        // timeout, and takes its request back.
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
        // A server that never takes it: given up after TAKE_WAIT, not the
        // whole timeout.
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

    #[test]
    fn a_signed_yes_approves_and_a_replay_or_a_no_does_not() {
        let dir = dir("answer");
        let phone = TestPhone::new("phone-a");
        let a = fresh("an-1");
        files::set_status(
            &dir,
            &a.id,
            &Status::Answered {
                device_id: "phone-a".to_string(),
                answer: Answer::Yes,
                signature: Some(phone.sign(&a)),
            },
        )
        .unwrap();
        assert_eq!(
            ask_and_wait(&dir, &a, &[phone.key()], |_| {}),
            Ok("phone-a".to_string())
        );
        // The same signature, offered for the next request.
        let b = Approval {
            nonce: "02".repeat(32),
            ..fresh("an-2")
        };
        files::set_status(
            &dir,
            &b.id,
            &Status::Answered {
                device_id: "phone-a".to_string(),
                answer: Answer::Yes,
                signature: Some(phone.sign(&a)),
            },
        )
        .unwrap();
        assert_eq!(
            ask_and_wait(&dir, &b, &[phone.key()], |_| {}),
            Err(GiveUp::Refused(Refusal::BadSignature))
        );
        let c = fresh("an-3");
        files::set_status(
            &dir,
            &c.id,
            &Status::Answered {
                device_id: "phone-a".to_string(),
                answer: Answer::No,
                signature: None,
            },
        )
        .unwrap();
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
            &Status::Answered {
                device_id: "phone-a".to_string(),
                answer: Answer::Yes,
                signature: Some(phone.sign(&a)),
            },
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
            precheck(Kind::Admin, true, &on(), true, &pin(&[&phone])),
            Err(GiveUp::Remote)
        );
        let mut from_afar = on();
        from_afar.phone_approves_local_only = false;
        assert!(precheck(Kind::Admin, true, &from_afar, true, &pin(&[&phone])).is_ok());
    }

    #[test]
    fn the_lock_screen_has_its_own_switch_and_unknown_services_are_refused() {
        assert_eq!(kind_of("sudo", None), Some(Kind::Admin));
        assert_eq!(kind_of("polkit-1", None), Some(Kind::Admin));
        assert_eq!(kind_of("hyprlock", None), Some(Kind::Unlock));
        assert_eq!(kind_of("login", None), None);
        assert_eq!(kind_of("sshd", None), None);
        assert_eq!(
            kind_of("pocketdesk-test", Some(Kind::Admin)),
            Some(Kind::Admin)
        );
        let phone = TestPhone::new("phone-a");
        assert_eq!(
            precheck(Kind::Unlock, false, &on(), true, &pin(&[&phone])),
            Err(GiveUp::LockScreenOff)
        );
        let mut lock = on();
        lock.phone_approves_lock_screen = true;
        assert!(precheck(Kind::Unlock, false, &lock, true, &pin(&[&phone])).is_ok());
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
}
