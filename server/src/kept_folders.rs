//! Phone folders kept on this computer, by themselves (the phone's
//! "Folders kept on my computer").
//!
//! A kept folder's files arrive as ordinary shares (`share.rs`: the
//! resumable transfer, its whole-file SHA-256), marked `keep`. This module
//! decides where such a file goes and whether it may:
//!
//! - only one of the owner's own phones may keep folders here, and only
//!   with `kept_folders` on;
//! - the file lands under `<root>/<phone>/<folder>`, names chosen here once
//!   (the registry in `<state>/kept_folders.json`), never taken from the
//!   phone's path as is: every name in the path was checked
//!   (`core::protocol::relpath`), and the folder on disk has the last word
//!   (a link planted inside the kept folder cannot lead a file out);
//! - the file replaces its older copy (a mirror, not a pile of `(1)`s), but
//!   never a folder or a link;
//! - all kept folders together stay within `kept_folders_cap_gb`;
//! - nothing is ever deleted unless the phone's run asks to mirror its
//!   deletions and `kept_folders_allow_delete` is on, and then only plain
//!   files below that folder, never more than half of what it holds in one
//!   run and never more than [`MAX_DELETE_PER_RUN`];
//! - a file in flight holds its bytes against the cap ([`Reservation`]), so
//!   several at once cannot pass it together;
//! - a name that differs from another in that folder only by case is
//!   refused (Windows would make them one), and never deleted by the other's
//!   name;
//! - a list of folders this version cannot read is never written over.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use pocketdesk_core::local::config::Config;
use pocketdesk_core::local::kept_folders::{self as registry, KeptFolder};
use pocketdesk_core::local::pin::now_unix;
use pocketdesk_core::protocol::kept::{self, KeptFile, Run};

/// The registry is read, changed and replaced under this lock.
static LOCK: Mutex<()> = Mutex::new(());

/// The most entries a count of a kept folder walks, so a folder of millions
/// of files cannot stall a run's end.
const MAX_WALK: usize = 2_000_000;

/// The most entries of one folder looked at for a name that differs only by
/// case, so a folder of millions cannot stall a file.
const MAX_SCAN: usize = 50_000;

/// The most files a phone's run may delete in all.
const MAX_DELETE_PER_RUN: u64 = 1000;

/// This few deletions in a run are made whatever the folder holds: one file
/// deleted from a folder of two is an ordinary deletion.
const DELETE_FREE: u64 = 2;

/// Bytes the files now arriving hold against the cap, so files that arrive
/// together cannot pass it together.
static RESERVED: Mutex<u64> = Mutex::new(0);

/// What each phone folder's current run has deleted so far, by phone and
/// folder: a run's deletions come in packets of at most 200, and the limits
/// are for the run, not the packet. A run's start or end forgets it.
static RUN_DELETED: Mutex<Option<HashMap<(String, String), u64>>> = Mutex::new(None);

/// Bytes held against the cap for a file in flight; given back when the
/// transfer ends, whether it was placed or not (dropped with the last copy
/// of its [`Accepted`]).
#[derive(Debug)]
pub(crate) struct Reservation(u64);

impl Drop for Reservation {
    fn drop(&mut self) {
        let mut held = RESERVED.lock().unwrap_or_else(|e| e.into_inner());
        *held = held.saturating_sub(self.0);
    }
}

/// Where one kept file goes, decided by [`accept`].
#[derive(Debug, Clone)]
pub(crate) struct Accepted {
    /// The folder the file lands in (made).
    pub dir: PathBuf,
    /// The kept folder itself: what the file's place is checked to stay in,
    /// again right before the file is moved in.
    pub top: PathBuf,
    /// The file's own name.
    pub name: String,
    pub folder_id: String,
    /// The folder on the phone, as the phone names it.
    pub label: String,
    /// Whether a file of this name was there already, and its size.
    pub replaces: Option<u64>,
    /// The bytes held against the cap while the file arrives.
    pub _reserved: Arc<Reservation>,
}

/// Whether `device` is one of the owner's own phones.
fn is_owner_phone(device: &str) -> bool {
    crate::state::trust_store_path()
        .ok()
        .and_then(|path| pocketdesk_core::protocol::kdeconnect::TrustStore::read(path).ok())
        .is_some_and(|store| store.is_owner_phone(device))
}

/// The phone's name as the owner sees it, else a short form of its id.
fn phone_name(device: &str) -> String {
    let named = crate::state::trust_store_path()
        .ok()
        .and_then(|path| pocketdesk_core::protocol::kdeconnect::TrustStore::read(path).ok())
        .and_then(|store| store.name(device).map(str::to_string));
    named.unwrap_or_else(|| format!("phone {}", device.chars().take(6).collect::<String>()))
}

/// Why `device` may not keep folders here now, or `None` when it may.
fn not_allowed(device: &str, config: &Config) -> Option<&'static str> {
    if !config.kept_folders {
        return Some(kept::OFF);
    }
    (!is_owner_phone(device)).then_some(kept::NOT_OWNER)
}

/// The root the kept folders go under: the owner's folder when it can take
/// them (set, absolute, there, and not Pocketdesk's own), else
/// `Documents/Pocketdesk phones`.
fn root(config: &Config) -> Result<PathBuf, String> {
    let chosen = config.kept_folders_root.trim();
    if !chosen.is_empty()
        && let Some(dir) = crate::plugins::share::chosen_folder_for(chosen)
    {
        return Ok(dir);
    }
    let base = dirs::document_dir()
        .or_else(dirs::home_dir)
        .ok_or("no documents or home folder for the kept folders")?;
    Ok(base.join("Pocketdesk phones"))
}

/// Decide where `file` of `size` bytes from `device` goes, and whether it
/// may: the checks above, then the folders made. The file's place is
/// recorded the first time its folder is seen, so an error (the cap) shows
/// on the Files page even for a first file.
pub(crate) fn accept(
    device: &str,
    file: &KeptFile,
    size: u64,
    config: &Config,
) -> Result<Accepted, String> {
    if let Some(why) = not_allowed(device, config) {
        return Err(why.to_string());
    }
    let root = root(config)?;
    let state = crate::state::state_dir().map_err(|e| e.to_string())?;
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut list = registry::read_checked(&state).map_err(|e| e.0)?;
    let at = match list
        .iter()
        .position(|f| f.device_id == device && f.folder_id == file.folder_id)
    {
        Some(at) => at,
        None => {
            if list.len() >= registry::MAX_FOLDERS {
                return Err(format!(
                    "this computer keeps at most {} phone folders",
                    registry::MAX_FOLDERS
                ));
            }
            let entry = new_entry(&list, &root, device, file);
            list.push(entry);
            list.len() - 1
        }
    };
    // Where it was first made, even if the root has been changed since:
    // files already kept stay where they are, and so do later ones.
    let top = PathBuf::from(&list[at].path);
    if list[at].label != file.label {
        list[at].label = file.label.clone();
    }
    // What is held now and what is arriving now, together.
    let arriving = *RESERVED.lock().unwrap_or_else(|e| e.into_inner());
    let cap = registry::cap_refusal(
        registry::used_bytes(&list).saturating_add(arriving),
        size,
        existing_len(&top, &file.path).unwrap_or(0),
        config.kept_folders_cap_gb,
    );
    if let Some(why) = cap {
        registry::push_error(&mut list[at], now_unix(), &why);
        let _ = registry::write(&state, &list);
        notify_full(&why);
        return Err(why);
    }
    // Under the root this folder was made under, which a later change of
    // the setting does not move.
    let made_under = top
        .parent()
        .and_then(Path::parent)
        .map_or_else(|| root.clone(), Path::to_path_buf);
    let placed = place(&top, &made_under, &file.path);
    match placed {
        Ok((dir, name, replaces)) => {
            let _ = registry::write(&state, &list);
            // Held under the same lock as the check, so two files cannot
            // both pass it.
            *RESERVED.lock().unwrap_or_else(|e| e.into_inner()) += size;
            Ok(Accepted {
                dir,
                name,
                top,
                folder_id: file.folder_id.clone(),
                label: file.label.clone(),
                replaces,
                _reserved: Arc::new(Reservation(size)),
            })
        }
        Err(why) => {
            registry::push_error(&mut list[at], now_unix(), &why);
            let _ = registry::write(&state, &list);
            Err(why)
        }
    }
}

/// The registry's entry for a folder seen for the first time: the names
/// for the phone and the folder, free under the root and the phone.
fn new_entry(list: &[KeptFolder], root: &Path, device: &str, file: &KeptFile) -> KeptFolder {
    let phone = phone_name(device);
    // The phone's own folder, if it has one: its name stays whatever the
    // phone is called later.
    let phone_dir = list
        .iter()
        .find(|f| f.device_id == device)
        .map(|f| f.phone_dir.clone())
        .unwrap_or_else(|| {
            let taken: Vec<String> = list.iter().map(|f| f.phone_dir.clone()).collect();
            registry::unique_name_where(&taken, &kept::folder_name(&phone), &|n| {
                exists_like(root, n)
            })
        });
    let taken: Vec<String> = list
        .iter()
        .filter(|f| f.device_id == device)
        .map(|f| f.folder_dir.clone())
        .collect();
    let under_phone = root.join(&phone_dir);
    let folder_dir = registry::unique_name_where(&taken, &kept::folder_name(&file.label), &|n| {
        exists_like(&under_phone, n)
    });
    KeptFolder {
        device_id: device.to_string(),
        folder_id: file.folder_id.clone(),
        phone,
        label: file.label.clone(),
        path: root
            .join(&phone_dir)
            .join(&folder_dir)
            .display()
            .to_string(),
        phone_dir,
        folder_dir,
        files: 0,
        bytes: 0,
        last_unix: 0,
        last_sent: 0,
        last_failed: 0,
        deleted: 0,
        deletes_skipped: 0,
        mirror: false,
        errors: Vec::new(),
    }
}

/// Whether `parent` already holds a name like `name`, in any case (made by
/// hand, or by an earlier registry).
fn exists_like(parent: &Path, name: &str) -> bool {
    std::fs::read_dir(parent).is_ok_and(|entries| {
        entries
            .flatten()
            .take(MAX_SCAN)
            .any(|e| registry::fold(&e.file_name().to_string_lossy()) == registry::fold(name))
    })
}

/// Whether `meta` is a link: a symbolic link, or on Windows a junction or any
/// other reparse point, none of which a kept folder may have in its way.
fn is_link(meta: &std::fs::Metadata) -> bool {
    meta.file_type().is_symlink() || is_reparse_point(meta)
}

#[cfg(windows)]
fn is_reparse_point(meta: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn is_reparse_point(_: &std::fs::Metadata) -> bool {
    false
}

/// Refuse a link, or a file where a folder goes, anywhere from `top` down
/// the folders `within` names, looking at each before anything is made, so
/// no folder is made through one. Below the first name that is not there,
/// nothing exists to be a link.
fn no_links(top: &Path, within: &[String]) -> Result<(), String> {
    if std::fs::symlink_metadata(top).is_ok_and(|m| is_link(&m)) {
        return Err("a link is where the kept folder is".to_string());
    }
    let mut at = top.to_path_buf();
    for part in within {
        at.push(part);
        match std::fs::symlink_metadata(&at) {
            Ok(meta) if is_link(&meta) => {
                return Err("a link is in the file's folder".to_string());
            }
            Ok(meta) if !meta.is_dir() => {
                return Err("a file is where the file's folder goes".to_string());
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(())
}

/// Refuse a new `name` in `parent` that differs from a name there only by
/// case: Windows would make the two one file. A name that is there exactly
/// is the same file, and is not looked for.
fn no_case_clash(parent: &Path, name: &str) -> Result<(), String> {
    if cfg!(not(windows)) && std::fs::symlink_metadata(parent.join(name)).is_ok() {
        return Ok(());
    }
    let Ok(entries) = std::fs::read_dir(parent) else {
        return Ok(());
    };
    let folded = registry::fold(name);
    for entry in entries.flatten().take(MAX_SCAN) {
        let there = entry.file_name().to_string_lossy().into_owned();
        if there != name && registry::fold(&there) == folded {
            return Err(
                "another name in that folder differs from this one only by case".to_string(),
            );
        }
    }
    Ok(())
}

/// Whether `name` is in `dir` exactly as spelled and no other name there
/// differs from it only by case, so a delete by it can reach no other file.
fn only_this_spelling(dir: &Path, name: &str) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    let folded = registry::fold(name);
    let mut found = false;
    for entry in entries.flatten().take(MAX_SCAN) {
        let there = entry.file_name().to_string_lossy().into_owned();
        if there == name {
            found = true;
        } else if registry::fold(&there) == folded {
            return false;
        }
    }
    found
}

/// The size of the file at `path` under `top`, when it is a plain file.
fn existing_len(top: &Path, path: &[String]) -> Option<u64> {
    let meta = std::fs::symlink_metadata(join(top, path)).ok()?;
    meta.is_file().then_some(meta.len())
}

/// `top` with `path`'s names after it.
fn join(top: &Path, path: &[String]) -> PathBuf {
    path.iter()
        .fold(top.to_path_buf(), |dir, part| dir.join(part))
}

/// Make the folder `path` (names under `top`, the file's own name last)
/// lands in, and return it, the file's name, and the size of the file it
/// replaces. The disk has the last word: a folder that leads out of `top`
/// (a planted link), a link or a folder where the file goes, are refused.
fn place(
    top: &Path,
    root: &Path,
    path: &[String],
) -> Result<(PathBuf, String, Option<u64>), String> {
    let (name, within) = path.split_last().ok_or("no file name")?;
    // No link anywhere on the way, looked for before a folder is made.
    no_links(top, within)?;
    std::fs::create_dir_all(top).map_err(|e| format!("could not make {}: {e}", top.display()))?;
    // No name that differs only by case from one already in a folder on
    // the way, or in the file's own.
    let mut at = top.to_path_buf();
    for part in within {
        no_case_clash(&at, part)?;
        at.push(part);
    }
    no_case_clash(&at, name)?;
    let dir = join(top, within);
    std::fs::create_dir_all(&dir).map_err(|e| format!("could not make {}: {e}", dir.display()))?;
    let real_top = std::fs::canonicalize(top).map_err(|e| e.to_string())?;
    let real_dir = std::fs::canonicalize(&dir).map_err(|e| e.to_string())?;
    if !real_dir.starts_with(&real_top) {
        return Err("the file's folder leads out of the kept folder".to_string());
    }
    // The kept folder itself is under the root, even through a link.
    let real_root = std::fs::canonicalize(root).map_err(|e| e.to_string())?;
    if !real_top.starts_with(&real_root) {
        return Err("the kept folder leads out of the kept-folders root".to_string());
    }
    let replaces = match std::fs::symlink_metadata(dir.join(name)) {
        Ok(meta) if meta.is_file() => Some(meta.len()),
        Ok(meta) if meta.is_dir() => {
            return Err("a folder is where the file goes".to_string());
        }
        Ok(_) => return Err("a link is where the file goes".to_string()),
        Err(_) => None,
    };
    Ok((dir, name.clone(), replaces))
}

/// The file `accepted` described is in place: count it, and date the folder.
pub(crate) fn placed(device: &str, accepted: &Accepted, size: u64) {
    let Ok(state) = crate::state::state_dir() else {
        return;
    };
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let Ok(mut list) = registry::read_checked(&state) else {
        return;
    };
    if let Some(folder) = list
        .iter_mut()
        .find(|f| f.device_id == device && f.folder_id == accepted.folder_id)
    {
        folder.bytes = folder
            .bytes
            .saturating_sub(accepted.replaces.unwrap_or(0))
            .saturating_add(size);
        if accepted.replaces.is_none() {
            folder.files = folder.files.saturating_add(1);
        }
        folder.last_unix = now_unix();
        let _ = registry::write(&state, &list);
    }
}

/// Tell the owner the cap stopped a phone, at most once in 30 minutes.
fn notify_full(why: &str) {
    static LAST: Mutex<Option<Instant>> = Mutex::new(None);
    {
        let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
        if last.is_some_and(|at| at.elapsed() < Duration::from_secs(30 * 60)) {
            return;
        }
        *last = Some(Instant::now());
    }
    tracing::warn!("A phone's kept folder was refused: {why}");
    // Paused (K2a): on the Files page, not announced.
    if crate::paused::now() {
        return;
    }
    let body = format!("A phone's kept folders stopped: {why}.");
    std::thread::spawn(move || {
        crate::platform::notify(
            "Pocketdesk: kept folders full",
            &body,
            Duration::from_secs(8),
        );
    });
}

/// A phone's run: its deletions, if they may be made, and at its end the
/// folder counted again and the run recorded. Any packet for a folder this
/// computer never saw is ignored.
pub(crate) fn run(device: &str, body: &serde_json::Value, config: &Config) -> Result<(), String> {
    if let Some(why) = not_allowed(device, config) {
        return Err(why.to_string());
    }
    let run = kept::run_of(body)?;
    let state = crate::state::state_dir().map_err(|e| e.to_string())?;
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut list = registry::read_checked(&state).map_err(|e| e.0)?;
    let Some(folder) = list
        .iter_mut()
        .find(|f| f.device_id == device && f.folder_id == run.folder_id)
    else {
        // A run's start for a folder not kept yet: its first file makes it.
        if run.gone.is_empty() && run.empty.is_empty() && run.end.is_none() {
            return Ok(());
        }
        return Err("no such kept folder here".to_string());
    };
    apply_run(folder, &run, config.kept_folders_allow_delete);
    registry::write(&state, &list).map_err(|e| e.to_string())
}

/// What a run does to its folder's record, and to the disk.
fn apply_run(folder: &mut KeptFolder, run: &Run, computer_allows: bool) {
    let now = now_unix();
    folder.mirror = run.mirror;
    let key = (folder.device_id.clone(), folder.folder_id.clone());
    // A run's start (no deletions, no end) begins its count anew.
    if run.gone.is_empty() && run.empty.is_empty() && run.end.is_none() {
        forget_run_deleted(&key);
    }
    if !run.gone.is_empty() {
        let already = run_deleted(&key);
        let refusal = deletions_refusal(already, run.gone.len(), folder.files);
        if !registry::deletes_allowed(run.mirror, computer_allows) {
            folder.deletes_skipped = folder.deletes_skipped.saturating_add(run.gone.len() as u64);
        } else if let Some(why) = refusal {
            // Counts only: no name is logged.
            tracing::warn!(
                "A kept folder's run asked to delete {} files and was refused: {why}",
                run.gone.len()
            );
            folder.deletes_skipped = folder.deletes_skipped.saturating_add(run.gone.len() as u64);
            registry::push_error(folder, now, &format!("deletions refused: {why}"));
        } else {
            let top = PathBuf::from(&folder.path);
            let mut made = 0u64;
            for path in &run.gone {
                if delete_one(&top, path) {
                    made += 1;
                }
            }
            folder.deleted = folder.deleted.saturating_add(made);
            add_run_deleted(&key, run.gone.len() as u64);
        }
    }
    if !run.empty.is_empty() {
        let top = PathBuf::from(&folder.path);
        for path in &run.empty {
            if let Some(size_before) = make_empty(&top, path) {
                if size_before.is_none() {
                    folder.files = folder.files.saturating_add(1);
                }
                folder.bytes = folder.bytes.saturating_sub(size_before.unwrap_or(0));
            }
        }
    }
    if let Some(end) = run.end {
        forget_run_deleted(&key);
        let (files, bytes) = count(Path::new(&folder.path));
        folder.files = files;
        folder.bytes = bytes;
        folder.last_unix = now;
        folder.last_sent = end.sent;
        folder.last_failed = end.failed;
        if end.failed > 0 {
            registry::push_error(
                folder,
                now,
                &format!(
                    "{} files could not be sent on the phone's last run",
                    end.failed
                ),
            );
        }
    }
}

fn run_deleted(key: &(String, String)) -> u64 {
    let held = RUN_DELETED.lock().unwrap_or_else(|e| e.into_inner());
    held.as_ref().and_then(|m| m.get(key)).copied().unwrap_or(0)
}

fn add_run_deleted(key: &(String, String), n: u64) {
    let mut held = RUN_DELETED.lock().unwrap_or_else(|e| e.into_inner());
    let entry = held
        .get_or_insert_with(HashMap::new)
        .entry(key.clone())
        .or_insert(0);
    *entry = entry.saturating_add(n);
}

fn forget_run_deleted(key: &(String, String)) {
    let mut held = RUN_DELETED.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(map) = held.as_mut() {
        map.remove(key);
    }
}

/// Why a run that has deleted `already` files may not delete `now` more from
/// a folder holding `kept` files, or `None` when it may: never more than
/// [`MAX_DELETE_PER_RUN`] in all, and never more than half of what the
/// folder holds (the phone checks this too; a phone that does not, or a
/// confused one, is stopped here), unless [`DELETE_FREE`] or fewer.
fn deletions_refusal(already: u64, now: usize, kept: u64) -> Option<String> {
    let total = already.saturating_add(now as u64);
    if total > MAX_DELETE_PER_RUN {
        return Some(format!(
            "a run may delete at most {MAX_DELETE_PER_RUN} files, and this one asked for {total}"
        ));
    }
    if total > DELETE_FREE && total.saturating_mul(2) > kept {
        return Some(format!(
            "{total} deletions in one run are more than half of the {kept} files kept"
        ));
    }
    None
}

/// Make the file at `path` under `top` empty (a phone's file of no bytes,
/// which a share cannot carry), made if it was not there; the size it had,
/// `Some(None)` when it was new, and `None` when it was refused (a link, a
/// folder, a name that differs only by case, a place that leads out).
fn make_empty(top: &Path, path: &[String]) -> Option<Option<u64>> {
    let made_under = top.parent().and_then(Path::parent)?;
    let (dir, name, replaces) = place(top, made_under, path).ok()?;
    std::fs::File::create(dir.join(name)).ok()?;
    Some(replaces)
}

/// Delete the plain file at `path` under `top`, then the folders it leaves
/// empty up to (not including) `top`; whether a file went. A folder or a
/// link at the place, or a place that leads out of `top`, is left alone.
fn delete_one(top: &Path, path: &[String]) -> bool {
    let Some((name, within)) = path.split_last() else {
        return false;
    };
    let Ok(real_top) = std::fs::canonicalize(top) else {
        return false;
    };
    let dir = join(top, within);
    let Ok(real_dir) = std::fs::canonicalize(&dir) else {
        return false;
    };
    if !real_dir.starts_with(&real_top) {
        return false;
    }
    // By the name as spelled, and by no other that differs only by case.
    if !only_this_spelling(&real_dir, name) {
        return false;
    }
    let target = real_dir.join(name);
    match std::fs::symlink_metadata(&target) {
        Ok(meta) if meta.is_file() => {}
        _ => return false,
    }
    if std::fs::remove_file(&target).is_err() {
        return false;
    }
    let mut parent = real_dir;
    while parent != real_top && parent.starts_with(&real_top) {
        // Fails while the folder holds anything, which ends the climb.
        if std::fs::remove_dir(&parent).is_err() {
            break;
        }
        if !parent.pop() {
            break;
        }
    }
    true
}

/// Files and bytes under `dir`, never following a link, bounded.
fn count(dir: &Path) -> (u64, u64) {
    let (mut files, mut bytes, mut seen) = (0u64, 0u64, 0usize);
    let mut pending = vec![dir.to_path_buf()];
    while let Some(next) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&next) else {
            continue;
        };
        for entry in entries.flatten() {
            seen += 1;
            if seen > MAX_WALK {
                return (files, bytes);
            }
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                pending.push(entry.path());
            } else if kind.is_file() {
                files += 1;
                bytes = bytes.saturating_add(entry.metadata().map(|m| m.len()).unwrap_or(0));
            }
        }
    }
    (files, bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pocketdesk_core::protocol::kept::RunEnd;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pd-kept-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn path(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|p| p.to_string()).collect()
    }

    fn entry(path: &Path) -> KeptFolder {
        KeptFolder {
            device_id: "phone".to_string(),
            folder_id: "0".repeat(32),
            phone: "Phone".to_string(),
            label: "Notes".to_string(),
            path: path.display().to_string(),
            phone_dir: "Phone".to_string(),
            folder_dir: "Notes".to_string(),
            files: 0,
            bytes: 0,
            last_unix: 0,
            last_sent: 0,
            last_failed: 0,
            deleted: 0,
            deletes_skipped: 0,
            mirror: false,
            errors: Vec::new(),
        }
    }

    fn a_run(mirror: bool, gone: &[&[&str]], end: Option<RunEnd>) -> Run {
        Run {
            folder_id: "0".repeat(32),
            label: "Notes".to_string(),
            mirror,
            gone: gone.iter().map(|p| path(p)).collect(),
            empty: Vec::new(),
            end,
        }
    }

    #[test]
    fn a_file_is_placed_under_its_folder_and_replaces_its_older_copy() {
        let root = temp_dir("place");
        let top = root.join("Phone").join("Notes");
        let (dir, name, replaces) = place(&top, &root, &path(&["a", "b.txt"])).unwrap();
        assert_eq!(dir, top.join("a"));
        assert_eq!(name, "b.txt");
        assert_eq!(replaces, None);
        std::fs::write(dir.join(&name), b"old bytes").unwrap();
        let (_, _, replaces) = place(&top, &root, &path(&["a", "b.txt"])).unwrap();
        assert_eq!(replaces, Some(9), "the older copy's size");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_folder_or_a_link_where_a_file_goes_is_refused() {
        let root = temp_dir("inway");
        let top = root.join("Phone").join("Notes");
        std::fs::create_dir_all(top.join("a")).unwrap();
        let err = place(&top, &root, &path(&["a"])).unwrap_err();
        assert!(err.contains("folder"), "{err}");
        #[cfg(unix)]
        {
            let outside = temp_dir("inway-out");
            std::fs::write(outside.join("secret"), b"x").unwrap();
            std::os::unix::fs::symlink(outside.join("secret"), top.join("link")).unwrap();
            let err = place(&top, &root, &path(&["link"])).unwrap_err();
            assert!(err.contains("link"), "{err}");
            let _ = std::fs::remove_dir_all(&outside);
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn a_link_inside_the_kept_folder_cannot_lead_a_file_out() {
        let root = temp_dir("escape");
        let top = root.join("Phone").join("Notes");
        let outside = temp_dir("escape-out");
        std::fs::create_dir_all(&top).unwrap();
        std::os::unix::fs::symlink(&outside, top.join("out")).unwrap();
        let err = place(&top, &root, &path(&["out", "evil.txt"])).unwrap_err();
        assert!(err.contains("link"), "{err}");
        assert!(!outside.join("evil.txt").exists());
        // A kept folder that is itself a link out of the root is refused too.
        let top2 = root.join("Phone").join("Linked");
        std::os::unix::fs::symlink(&outside, &top2).unwrap();
        assert!(place(&top2, &root, &path(&["x.txt"])).is_err());
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&outside);
    }

    #[test]
    fn a_new_folder_gets_names_free_under_the_root_and_its_phone() {
        let _state = crate::state::use_test_dirs();
        let root = Path::new("/kept");
        let file = |id: &str, label: &str| KeptFile {
            folder_id: id.to_string(),
            label: label.to_string(),
            path: path(&["x"]),
        };
        let first = new_entry(&[], root, "phone-a", &file(&"1".repeat(32), "Notes"));
        assert_eq!(first.folder_dir, "Notes");
        // The same phone, another folder named alike: its own free name; the
        // phone's own folder is kept, whatever the phone is called.
        let second = new_entry(
            std::slice::from_ref(&first),
            root,
            "phone-a",
            &file(&"2".repeat(32), "notes"),
        );
        assert_eq!(second.phone_dir, first.phone_dir);
        assert_eq!(second.folder_dir, "notes (2)");
        // Another phone that would take the same folder name gets another.
        let other = new_entry(
            &[first.clone(), second],
            root,
            "phone-b",
            &file(&"3".repeat(32), "Notes"),
        );
        assert_eq!(other.folder_dir, "Notes");
        assert_ne!(
            other.phone_dir.to_lowercase(),
            first.phone_dir.to_lowercase()
        );
        // A phone that names its folder like a path gets a safe name.
        let odd = new_entry(&[], root, "phone-c", &file(&"4".repeat(32), "../../etc"));
        assert_eq!(
            pocketdesk_core::protocol::relpath::component_problem(&odd.folder_dir),
            None,
            "one safe name: {}",
            odd.folder_dir
        );
        assert!(!odd.folder_dir.contains('/'));
        assert!(!odd.folder_dir.contains(r"\"));
        assert!(Path::new(&odd.path).starts_with(root));
    }

    #[test]
    fn a_delete_removes_a_plain_file_and_the_folders_it_leaves_empty() {
        let root = temp_dir("delete");
        let top = root.join("Notes");
        std::fs::create_dir_all(top.join("a").join("b")).unwrap();
        std::fs::write(top.join("a").join("b").join("one.txt"), b"1").unwrap();
        std::fs::write(top.join("a").join("two.txt"), b"2").unwrap();
        std::fs::write(top.join("keep.txt"), b"k").unwrap();
        assert!(delete_one(&top, &path(&["a", "b", "one.txt"])));
        assert!(!top.join("a").join("b").exists(), "the empty folder went");
        assert!(
            top.join("a").join("two.txt").exists(),
            "a folder with a file stays"
        );
        assert!(delete_one(&top, &path(&["a", "two.txt"])));
        assert!(!top.join("a").exists());
        assert!(
            top.exists() && top.join("keep.txt").exists(),
            "never the top itself"
        );
        assert!(!delete_one(&top, &path(&["nothing.txt"])), "no such file");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_delete_never_leaves_the_kept_folder_nor_removes_a_folder() {
        let root = temp_dir("delete-safe");
        let top = root.join("Notes");
        let beside = root.join("Other");
        std::fs::create_dir_all(top.join("sub")).unwrap();
        std::fs::create_dir_all(&beside).unwrap();
        std::fs::write(beside.join("x.txt"), b"x").unwrap();
        // The phone's names cannot climb (checked on arrival), and the
        // delete checks again.
        assert!(!delete_one(&top, &path(&["..", "Other", "x.txt"])));
        assert!(beside.join("x.txt").exists());
        // A folder is not a file.
        assert!(!delete_one(&top, &path(&["sub"])));
        assert!(top.join("sub").exists());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(beside.join("x.txt"), top.join("link")).unwrap();
            assert!(!delete_one(&top, &path(&["link"])), "a link is left alone");
            assert!(beside.join("x.txt").exists());
            std::os::unix::fs::symlink(&beside, top.join("dirlink")).unwrap();
            assert!(!delete_one(&top, &path(&["dirlink", "x.txt"])));
            assert!(beside.join("x.txt").exists());
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn deletions_need_the_phone_and_the_computer_both() {
        let root = temp_dir("both");
        let top = root.join("Notes");
        std::fs::create_dir_all(&top).unwrap();
        let make = || {
            std::fs::write(top.join("a.txt"), b"a").unwrap();
            entry(&top)
        };
        // The phone does not ask: nothing is deleted, nothing counted skipped
        // beyond what it named.
        let mut folder = make();
        apply_run(&mut folder, &a_run(false, &[&["a.txt"]], None), true);
        assert!(top.join("a.txt").exists());
        assert_eq!((folder.deleted, folder.deletes_skipped), (0, 1));
        // The phone asks and this computer does not allow it.
        let mut folder = make();
        apply_run(&mut folder, &a_run(true, &[&["a.txt"]], None), false);
        assert!(top.join("a.txt").exists());
        assert_eq!((folder.deleted, folder.deletes_skipped), (0, 1));
        // Both.
        let mut folder = make();
        apply_run(&mut folder, &a_run(true, &[&["a.txt"]], None), true);
        assert!(!top.join("a.txt").exists());
        assert_eq!((folder.deleted, folder.deletes_skipped), (1, 0));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_runs_end_counts_the_folder_again_and_records_the_run() {
        let root = temp_dir("end");
        let top = root.join("Notes");
        std::fs::create_dir_all(top.join("d")).unwrap();
        std::fs::write(top.join("a.bin"), vec![0u8; 10]).unwrap();
        std::fs::write(top.join("d").join("b.bin"), vec![0u8; 5]).unwrap();
        let mut folder = entry(&top);
        folder.files = 99;
        folder.bytes = 99;
        apply_run(
            &mut folder,
            &a_run(false, &[], Some(RunEnd { sent: 2, failed: 1 })),
            false,
        );
        assert_eq!((folder.files, folder.bytes), (2, 15));
        assert_eq!((folder.last_sent, folder.last_failed), (2, 1));
        assert!(folder.last_unix > 0);
        assert_eq!(folder.errors.len(), 1, "a failed file is a problem to show");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_count_walks_files_and_bytes_without_following_links() {
        let root = temp_dir("count");
        std::fs::create_dir_all(root.join("a").join("b")).unwrap();
        std::fs::write(root.join("a").join("b").join("x"), vec![0u8; 7]).unwrap();
        std::fs::write(root.join("y"), vec![0u8; 3]).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&root, root.join("loop")).unwrap();
        assert_eq!(count(&root), (2, 10));
        assert_eq!(count(&root.join("missing")), (0, 0));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A config whose kept folders go under `root`, and the owner's phone
    /// `id` paired as one.
    fn setup(id: &str, root: &Path) -> Config {
        let trust = crate::state::trust_store_path().unwrap();
        pocketdesk_core::protocol::kdeconnect::TrustStore::update(trust, |s| {
            s.trust_phone(id, true)
        })
        .unwrap();
        Config {
            kept_folders_root: root.display().to_string(),
            ..Config::default()
        }
    }

    fn file(folder: &str, parts: &[&str]) -> KeptFile {
        KeptFile {
            folder_id: folder.to_string(),
            label: "Notes".to_string(),
            path: path(parts),
        }
    }

    #[test]
    fn only_an_owner_phone_keeps_folders_here_and_only_with_the_setting_on() {
        let _state = crate::state::use_test_dirs();
        let root = temp_dir("accept-who");
        let config = setup("kept-owner-who", &root);
        let id = "a".repeat(32);
        let f = file(&id, &["x.txt"]);
        // A device that is paired as no owner's phone, or not at all.
        let err = accept("kept-stranger-who", &f, 10, &config).unwrap_err();
        assert_eq!(err, kept::NOT_OWNER);
        assert!(!root.join("Notes").exists());
        assert!(accept("kept-owner-who", &f, 10, &config).is_ok());
        // Off on the computer: refused, whoever asks.
        let off = Config {
            kept_folders: false,
            ..config
        };
        assert_eq!(
            accept("kept-owner-who", &f, 10, &off).unwrap_err(),
            kept::OFF
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_kept_file_lands_in_its_folder_and_the_cap_stops_what_would_pass_it() {
        let _state = crate::state::use_test_dirs();
        let root = temp_dir("accept-cap");
        let mut config = setup("kept-owner-cap", &root);
        let id = "b".repeat(32);
        let first = accept(
            "kept-owner-cap",
            &file(&id, &["d", "one.bin"]),
            100,
            &config,
        )
        .unwrap();
        assert!(first.dir.starts_with(&root), "{:?}", first.dir);
        assert!(first.dir.ends_with("d"));
        assert_eq!(first.name, "one.bin");
        assert_eq!(first.replaces, None);
        std::fs::write(first.dir.join(&first.name), vec![0u8; 100]).unwrap();
        placed("kept-owner-cap", &first, 100);
        let again = accept(
            "kept-owner-cap",
            &file(&id, &["d", "one.bin"]),
            120,
            &config,
        )
        .unwrap();
        assert_eq!(again.replaces, Some(100));
        assert_eq!(again.dir, first.dir, "the same place");
        // The folder holds more than a 1 GB cap allows now.
        let big = Accepted {
            replaces: None,
            ..first.clone()
        };
        placed("kept-owner-cap", &big, 2 * 1024 * 1024 * 1024);
        config.kept_folders_cap_gb = 1;
        let why = accept("kept-owner-cap", &file(&id, &["d", "two.bin"]), 1, &config).unwrap_err();
        assert!(why.starts_with(kept::FULL), "{why}");
        // The Files page is told, even though nothing was kept.
        let state = crate::state::state_dir().unwrap();
        let seen = registry::read(&state)
            .into_iter()
            .find(|f| f.device_id == "kept-owner-cap")
            .unwrap();
        assert!(
            seen.errors
                .iter()
                .any(|e| e.message.starts_with(kept::FULL))
        );
        // No limit at 0.
        config.kept_folders_cap_gb = 0;
        assert!(accept("kept-owner-cap", &file(&id, &["d", "two.bin"]), 1, &config).is_ok());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_runs_deletion_by_the_server_needs_the_computers_setting_too() {
        let _state = crate::state::use_test_dirs();
        let root = temp_dir("run-delete");
        let mut config = setup("kept-owner-del", &root);
        let id = "c".repeat(32);
        let kept = accept("kept-owner-del", &file(&id, &["gone.txt"]), 3, &config).unwrap();
        std::fs::write(kept.dir.join(&kept.name), b"abc").unwrap();
        placed("kept-owner-del", &kept, 3);
        let body = serde_json::json!({
            "folderId": id, "folder": "Notes", "mirror": true,
            "gone": [["gone.txt"]], "done": true, "sent": 0, "failed": 0,
        });
        run("kept-owner-del", &body, &config).unwrap();
        assert!(kept.dir.join("gone.txt").exists(), "the setting is off");
        config.kept_folders_allow_delete = true;
        run("kept-owner-del", &body, &config).unwrap();
        assert!(!kept.dir.join("gone.txt").exists());
        // Someone else's run, or an unknown folder, deletes nothing.
        assert!(run("kept-stranger-del", &body, &config).is_err());
        let other = serde_json::json!({"folderId": "d".repeat(32), "gone": [["x"]]});
        assert!(run("kept-owner-del", &other, &config).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn deletions_are_limited_to_half_of_what_is_kept_and_a_number_for_the_run() {
        assert_eq!(deletions_refusal(0, 1, 10), None);
        assert_eq!(deletions_refusal(0, 5, 10), None, "exactly half");
        assert!(deletions_refusal(0, 6, 10).is_some());
        assert_eq!(deletions_refusal(0, 2, 2), None, "the free few");
        assert!(deletions_refusal(0, 3, 4).is_some());
        // A run's packets add up.
        assert_eq!(deletions_refusal(0, 4, 10), None);
        assert!(deletions_refusal(4, 4, 10).is_some());
        // And a number for the run, however large the folder.
        assert_eq!(deletions_refusal(0, 1000, 100_000), None);
        assert!(deletions_refusal(0, 1001, 100_000).is_some());
        assert!(deletions_refusal(900, 101, u64::MAX).is_some());
        // Nothing overflows.
        assert!(deletions_refusal(u64::MAX, usize::MAX, 0).is_some());
    }

    #[test]
    fn a_run_cannot_delete_most_of_a_folder_even_when_the_phone_says_to() {
        let root = temp_dir("guard");
        let top = root.join("Notes");
        std::fs::create_dir_all(&top).unwrap();
        let fill = || {
            for n in 0..6 {
                std::fs::write(top.join(format!("f{n}")), b"x").unwrap();
            }
        };
        fill();
        let mut folder = entry(&top);
        folder.device_id = "kept-guard-phone".to_string();
        folder.files = 6;
        // Five of six in one packet: refused, every file stays, and it says so.
        let five = a_run(true, &[&["f0"], &["f1"], &["f2"], &["f3"], &["f4"]], None);
        apply_run(&mut folder, &five, true);
        assert!((0..6).all(|n| top.join(format!("f{n}")).exists()));
        assert_eq!((folder.deleted, folder.deletes_skipped), (0, 5));
        assert!(
            folder
                .errors
                .iter()
                .any(|e| e.message.contains("refused") && !e.message.contains("f0"))
        );
        // A run's packets count together: two and two is four of six.
        apply_run(&mut folder, &a_run(true, &[], None), true);
        apply_run(&mut folder, &a_run(true, &[&["f0"], &["f1"]], None), true);
        assert_eq!(folder.deleted, 2);
        apply_run(&mut folder, &a_run(true, &[&["f2"], &["f3"]], None), true);
        assert!(
            top.join("f2").exists() && top.join("f3").exists(),
            "the second packet is over half"
        );
        assert_eq!(folder.deleted, 2);
        // The run's end counts the folder again and a new run starts afresh.
        apply_run(
            &mut folder,
            &a_run(true, &[], Some(RunEnd { sent: 0, failed: 0 })),
            true,
        );
        assert_eq!(folder.files, 4);
        apply_run(&mut folder, &a_run(true, &[&["f2"]], None), true);
        assert!(!top.join("f2").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn a_link_anywhere_on_the_way_is_refused_before_a_folder_is_made() {
        let root = temp_dir("links");
        let top = root.join("Phone").join("Notes");
        let outside = temp_dir("links-out");
        std::fs::create_dir_all(top.join("real")).unwrap();
        // A link to a folder inside the kept folder: it stays inside, and is
        // still refused (what is checked is the path, not only where it ends).
        std::os::unix::fs::symlink(top.join("real"), top.join("alias")).unwrap();
        let err = place(&top, &root, &path(&["alias", "x.txt"])).unwrap_err();
        assert!(err.contains("link"), "{err}");
        assert!(!top.join("real").join("x.txt").exists());
        // A link out, with new names below it: nothing is made through it.
        std::os::unix::fs::symlink(&outside, top.join("out")).unwrap();
        let err = place(&top, &root, &path(&["out", "b", "c.txt"])).unwrap_err();
        assert!(err.contains("link"), "{err}");
        assert!(!outside.join("b").exists(), "no folder made through a link");
        // A file where a folder goes.
        std::fs::write(top.join("plain"), b"x").unwrap();
        assert!(place(&top, &root, &path(&["plain", "y.txt"])).is_err());
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&outside);
    }

    #[test]
    fn a_name_that_differs_only_by_case_is_refused_and_never_deleted_by_the_other() {
        let root = temp_dir("case");
        let top = root.join("Phone").join("Notes");
        std::fs::create_dir_all(top.join("Docs")).unwrap();
        std::fs::write(top.join("A.txt"), b"one").unwrap();
        // A second spelling of a file's name, and of a folder's.
        let err = place(&top, &root, &path(&["a.txt"])).unwrap_err();
        assert!(err.contains("case"), "{err}");
        let err = place(&top, &root, &path(&["docs", "x.txt"])).unwrap_err();
        assert!(err.contains("case"), "{err}");
        // The same spelling is the same file.
        assert!(place(&top, &root, &path(&["A.txt"])).is_ok());
        assert!(place(&top, &root, &path(&["Docs", "x.txt"])).is_ok());
        // Where both spellings exist (a disk that tells them apart), a delete
        // by either name removes neither.
        #[cfg(unix)]
        {
            std::fs::write(top.join("a.txt"), b"two").unwrap();
            assert!(!delete_one(&top, &path(&["a.txt"])));
            assert!(!delete_one(&top, &path(&["A.txt"])));
            assert!(top.join("a.txt").exists() && top.join("A.txt").exists());
        }
        // A name spelled otherwise than the file on the disk deletes nothing.
        std::fs::write(top.join("Only.txt"), b"x").unwrap();
        assert!(!delete_one(&top, &path(&["only.txt"])));
        assert!(top.join("Only.txt").exists());
        assert!(delete_one(&top, &path(&["Only.txt"])));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn files_arriving_together_cannot_pass_the_cap_together() {
        let state = crate::state::use_test_dirs();
        // This test's own folders only: the list is shared by the process.
        let _ = std::fs::remove_file(state.join(pocketdesk_core::local::paths::KEPT_FOLDERS_FILE));
        let root = temp_dir("reserve");
        let mut config = setup("kept-owner-reserve", &root);
        config.kept_folders_cap_gb = 1;
        const MB: u64 = 1024 * 1024;
        let id = "f".repeat(32);
        let first = accept(
            "kept-owner-reserve",
            &file(&id, &["a.bin"]),
            600 * MB,
            &config,
        )
        .unwrap();
        let second = accept(
            "kept-owner-reserve",
            &file(&id, &["b.bin"]),
            600 * MB,
            &config,
        );
        assert!(
            second.unwrap_err().starts_with(kept::FULL),
            "600 and 600 MB are over 1 GB"
        );
        // Given back when the transfer ends, placed or not.
        drop(first);
        assert!(
            accept(
                "kept-owner-reserve",
                &file(&id, &["b.bin"]),
                600 * MB,
                &config
            )
            .is_ok()
        );
        let _ = std::fs::remove_file(state.join(pocketdesk_core::local::paths::KEPT_FOLDERS_FILE));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_list_this_version_cannot_read_is_kept_and_nothing_is_written_over_it() {
        let state = crate::state::use_test_dirs();
        let list = state.join(pocketdesk_core::local::paths::KEPT_FOLDERS_FILE);
        let root = temp_dir("unreadable");
        let config = setup("kept-owner-unreadable", &root);
        std::fs::write(&list, "{ records of another version").unwrap();
        let id = "9".repeat(32);
        let err = accept("kept-owner-unreadable", &file(&id, &["x.txt"]), 1, &config).unwrap_err();
        assert!(err.contains("not readable"), "{err}");
        let body = serde_json::json!({"folderId": id, "done": true, "sent": 0, "failed": 0});
        assert!(run("kept-owner-unreadable", &body, &config).is_err());
        assert_eq!(
            std::fs::read_to_string(&list).unwrap(),
            "{ records of another version",
            "the records are as they were"
        );
        assert!(!root.join("Notes").exists());
        let _ = std::fs::remove_file(&list);
        let _ = std::fs::remove_file(format!("{}.unreadable", list.display()));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_file_of_no_bytes_is_made_or_emptied_and_never_through_a_link() {
        let root = temp_dir("empties");
        let top = root.join("Notes");
        std::fs::create_dir_all(&top).unwrap();
        std::fs::write(top.join("e.txt"), b"hello").unwrap();
        let mut folder = entry(&top);
        folder.files = 1;
        folder.bytes = 5;
        let told = Run {
            empty: vec![path(&["e.txt"]), path(&["sub", "n.txt"])],
            ..a_run(false, &[], None)
        };
        apply_run(&mut folder, &told, false);
        assert_eq!(
            std::fs::metadata(top.join("e.txt")).unwrap().len(),
            0,
            "emptied"
        );
        assert_eq!(
            std::fs::metadata(top.join("sub").join("n.txt"))
                .unwrap()
                .len(),
            0,
            "made"
        );
        assert_eq!((folder.files, folder.bytes), (2, 0));
        #[cfg(unix)]
        {
            let outside = temp_dir("empties-out");
            std::fs::write(outside.join("secret"), b"keep me").unwrap();
            std::os::unix::fs::symlink(outside.join("secret"), top.join("link")).unwrap();
            let told = Run {
                empty: vec![path(&["link"])],
                ..a_run(false, &[], None)
            };
            apply_run(&mut folder, &told, false);
            assert_eq!(std::fs::read(outside.join("secret")).unwrap(), b"keep me");
            let _ = std::fs::remove_dir_all(&outside);
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}
