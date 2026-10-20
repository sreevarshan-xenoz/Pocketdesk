//! Windows implementations of [`super`]'s helpers.

use windows::Data::Xml::Dom::XmlDocument;
use windows::Foundation::{IPropertyValue, TypedEventHandler};
use windows::UI::Notifications::{
    ToastActivatedEventArgs, ToastNotification, ToastNotificationManager,
};
use windows::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey,
    RegCreateKeyExW, RegDeleteTreeW, RegSetValueExW,
};
use windows::Win32::System::Threading::{
    CreateEventW, EVENT_MODIFY_STATE, INFINITE, OpenEventW, OpenProcess, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE, QueryFullProcessImageNameW, SetEvent,
    TerminateProcess, WaitForSingleObject,
};
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};
use windows::core::{HSTRING, IInspectable, Interface, PCWSTR, w};

/// The identity toasts are shown under. An unpackaged app needs one registered
/// under `HKCU\Software\Classes\AppUserModelId` for Windows to display them.
const AUMID: &str = "Pocketdesk.Server";

/// A tool in the real System32, located by asking Windows (not `%SystemRoot%`,
/// which the user can point anywhere).
pub(crate) fn system32(tool: &str) -> std::path::PathBuf {
    let mut buffer = [0u16; 260];
    let len = unsafe {
        windows::Win32::System::SystemInformation::GetSystemDirectoryW(Some(&mut buffer))
    } as usize;
    let dir = if len > 0 && len < buffer.len() {
        std::path::PathBuf::from(String::from_utf16_lossy(&buffer[..len]))
    } else {
        std::path::PathBuf::from(r"C:\Windows\System32")
    };
    dir.join(tool)
}

pub(super) fn init_process() {
    // Fails harmlessly when awareness was already set (e.g. by a manifest),
    // which is the only way it can fail on a supported Windows.
    if let Err(e) =
        unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) }
    {
        tracing::debug!("DPI awareness not changed: {e}");
    }
    // Hold the multithreaded apartment for the process's life (R6g review):
    // threads enter and leave COM (capture, sound, toasts), and once the last
    // had left, COM could unload a library a cached WinRT factory still
    // points into (windows-core keeps those factories for good).
    // GraphicsCapture.dll is pinned besides; this covers the others.
    if let Err(e) = unsafe { windows::Win32::System::Com::CoIncrementMTAUsage() } {
        tracing::warn!("The COM apartment is not held for the process: {e}");
    }
}

/// Every module this process has had loaded, as last seen: base, size and
/// file name. A fault executing an address in no module now loaded (code
/// unloaded under a callback) is named from here.
static MODULES: std::sync::Mutex<Vec<(usize, usize, String)>> = std::sync::Mutex::new(Vec::new());

/// Note the modules loaded now. One seen before stays listed, unless a
/// module now loaded overlaps it (its range was reused).
fn remember_modules() {
    use windows::Win32::Foundation::HMODULE;
    use windows::Win32::System::ProcessStatus::{
        EnumProcessModules, GetModuleFileNameExW, GetModuleInformation, MODULEINFO,
    };
    use windows::Win32::System::Threading::GetCurrentProcess;
    let mut handles = vec![HMODULE::default(); 1024];
    let mut needed = 0u32;
    // SAFETY: a buffer of HMODULEs and its size in bytes.
    let listed = unsafe {
        EnumProcessModules(
            GetCurrentProcess(),
            handles.as_mut_ptr(),
            (handles.len() * std::mem::size_of::<HMODULE>()) as u32,
            &mut needed,
        )
    };
    if listed.is_err() {
        return;
    }
    let count = (needed as usize / std::mem::size_of::<HMODULE>()).min(handles.len());
    let mut seen = Vec::with_capacity(count);
    for &module in &handles[..count] {
        let mut info = MODULEINFO::default();
        // SAFETY: a module of this process and a MODULEINFO to fill.
        if unsafe {
            GetModuleInformation(
                GetCurrentProcess(),
                module,
                &mut info,
                std::mem::size_of::<MODULEINFO>() as u32,
            )
        }
        .is_err()
        {
            continue;
        }
        let mut name = [0u16; 260];
        // SAFETY: the same module, and a buffer for its path.
        let len =
            unsafe { GetModuleFileNameExW(Some(GetCurrentProcess()), Some(module), &mut name) };
        let path = String::from_utf16_lossy(&name[..len as usize]);
        let file = path.rsplit('\\').next().unwrap_or(&path).to_string();
        seen.push((info.lpBaseOfDll as usize, info.SizeOfImage as usize, file));
    }
    if let Ok(mut modules) = MODULES.lock() {
        modules.retain(|old| {
            seen.contains(old)
                || !seen
                    .iter()
                    .any(|new| old.0 < new.0 + new.1 && new.0 < old.0 + old.1)
        });
        for module in seen {
            if !modules.contains(&module) {
                modules.push(module);
            }
        }
    }
}

/// Write where `address` is: `name+offset` of the module last seen there.
/// Never blocks and never allocates (it runs while an exception is
/// dispatched, maybe on a thread holding the heap or loader lock).
fn write_place(out: &mut impl std::io::Write, address: usize) {
    let Ok(modules) = MODULES.try_lock() else {
        let _ = write!(out, "{address:#x} (modules busy)");
        return;
    };
    match modules
        .iter()
        .find(|(base, size, _)| (*base..base + size).contains(&address))
    {
        Some((base, _, name)) => {
            let _ = write!(out, "{address:#x} in {name}+{:#x}", address - base);
        }
        None => {
            let _ = write!(out, "{address:#x} in no module seen loaded");
        }
    }
}

/// What the memory at `address` is now: free (nothing mapped: code since
/// unloaded), part of a loaded image, or other.
fn mapping(address: usize) -> &'static str {
    use windows::Win32::System::Memory::{
        MEM_FREE, MEM_IMAGE, MEMORY_BASIC_INFORMATION, VirtualQuery,
    };
    let mut info = MEMORY_BASIC_INFORMATION::default();
    // SAFETY: a query of this process's own address space into a local.
    let filled = unsafe {
        VirtualQuery(
            Some(address as *const _),
            &mut info,
            std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
        )
    };
    if filled == 0 || info.State == MEM_FREE {
        "unmapped"
    } else if info.Type == MEM_IMAGE {
        "image"
    } else {
        "not an image"
    }
}

/// The exception in words: its code, where and what is there now, and for
/// an access violation what was read, written or executed. Allocates
/// nothing.
fn describe(
    out: &mut impl std::io::Write,
    record: &windows::Win32::System::Diagnostics::Debug::EXCEPTION_RECORD,
) {
    let code = record.ExceptionCode.0 as u32;
    let address = record.ExceptionAddress as usize;
    let _ = write!(out, "exception {code:#010x} at ");
    write_place(out, address);
    let _ = write!(out, " ({})", mapping(address));
    if code == 0xC000_0005 && record.NumberParameters >= 2 {
        let verb = match record.ExceptionInformation[0] {
            0 => "reading",
            1 => "writing",
            8 => "executing",
            _ => "touching",
        };
        let _ = write!(out, " ({verb} {:#x})", record.ExceptionInformation[1]);
    }
}

/// The crash log, opened at start for the first-chance reports, which may
/// not open files.
static FIRST_CHANCE_LOG: std::sync::OnceLock<std::fs::File> = std::sync::OnceLock::new();

/// Report an unhandled exception (an access violation, say) to the crash
/// log before Windows ends the process, as it still does; and the first
/// few faults of a run as they are raised, which also catches those that
/// end a process already exiting.
pub(crate) fn install_exception_report() {
    use windows::Win32::System::Diagnostics::Debug::{
        AddVectoredExceptionHandler, EXCEPTION_POINTERS, RtlCaptureStackBackTrace,
        SetUnhandledExceptionFilter,
    };

    unsafe extern "system" fn filter(info: *const EXCEPTION_POINTERS) -> i32 {
        // SAFETY: Windows passes the exception being dispatched.
        let what = unsafe { info.as_ref().and_then(|i| i.ExceptionRecord.as_ref()) }
            .map(|record| {
                let mut line = Vec::new();
                describe(&mut line, record);
                String::from_utf8_lossy(&line).into_owned()
            })
            .unwrap_or_else(|| "an unhandled exception".to_string());
        crate::crash_report::report(&what);
        // EXCEPTION_CONTINUE_SEARCH: the process ends as it would have.
        0
    }

    /// First-chance faults: an access violation as the process exits (a
    /// thread still running while the runtime is torn down) never reaches
    /// the unhandled-exception filter, so the server's exit 139 left no
    /// report (reliability pass, 2026-10-16). Every exception passes here
    /// first. Reported, at most three a run: an illegal instruction, heap
    /// corruption, and an access violation executing, or at an address
    /// outside any loaded image (code unloaded under a thread); not one
    /// reading or writing inside loaded code, which drivers raise and
    /// handle as they probe. The report allocates nothing and takes no
    /// lock that can wait: the line and the raw return addresses go
    /// straight to the log opened at start. Each exception is passed on
    /// unchanged.
    unsafe extern "system" fn first_chance(info: *mut EXCEPTION_POINTERS) -> i32 {
        use std::io::Write;
        use std::sync::atomic::{AtomicU32, Ordering};
        static REPORTED: AtomicU32 = AtomicU32::new(0);
        // SAFETY: Windows passes the exception being dispatched.
        let Some(record) = (unsafe { info.as_ref().and_then(|i| i.ExceptionRecord.as_ref()) })
        else {
            return 0;
        };
        let code = record.ExceptionCode.0 as u32;
        let address = record.ExceptionAddress as usize;
        let executing = record.NumberParameters >= 1 && record.ExceptionInformation[0] == 8;
        let report = match code {
            0xC000_001D | 0xC000_0374 => true,
            0xC000_0005 => executing || mapping(address) != "image",
            _ => false,
        };
        if !report || REPORTED.fetch_add(1, Ordering::Relaxed) >= 3 {
            return 0;
        }
        let Some(mut log) = FIRST_CHANCE_LOG.get() else {
            return 0;
        };
        let mut buf = [0u8; 4096];
        let mut out = std::io::Cursor::new(&mut buf[..]);
        let unix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let _ = write!(
            out,
            "=== pocketdesk {} at unix {unix}: first-chance ",
            env!("CARGO_PKG_VERSION")
        );
        describe(&mut out, record);
        let _ = writeln!(out, " (it may yet have been handled)");
        let mut frames = [std::ptr::null_mut(); 24];
        // SAFETY: a buffer of frame pointers on this thread's stack.
        let depth = unsafe { RtlCaptureStackBackTrace(0, &mut frames, None) } as usize;
        for &frame in &frames[..depth.min(frames.len())] {
            let _ = write!(out, "  ");
            write_place(&mut out, frame as usize);
            let _ = writeln!(out);
        }
        let len = out.position() as usize;
        let _ = log.write_all(&buf[..len]);
        // EXCEPTION_CONTINUE_SEARCH: handled, or not, as it would have been.
        0
    }

    if let Some(path) = crate::crash_report::log_path()
        && let Ok(file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
    {
        let _ = FIRST_CHANCE_LOG.set(file);
    }
    unsafe {
        SetUnhandledExceptionFilter(Some(filter));
        AddVectoredExceptionHandler(1, Some(first_chance));
    }
    // Modules come and go with sessions (capture, audio, encoders): note
    // them now and every 30 s, so a fault in one since unloaded is named.
    remember_modules();
    let _ = std::thread::Builder::new()
        .name("module-watch".into())
        .spawn(|| {
            loop {
                std::thread::sleep(std::time::Duration::from_secs(30));
                remember_modules();
            }
        });
}

pub(super) fn detach_console() {
    let _ = unsafe { windows::Win32::System::Console::FreeConsole() };
}

/// Set the default value of `HKCU\<subkey>` to `value`, making the key.
pub(crate) fn set_user_default_value(subkey: &str, value: &str) -> windows::core::Result<()> {
    let subkey = HSTRING::from(subkey);
    let mut key = HKEY::default();
    unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            &subkey,
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            None,
            &mut key,
            None,
        )
        .ok()?;
        let data: Vec<u8> = format!("{value}\0")
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        let set = RegSetValueExW(key, PCWSTR::null(), None, REG_SZ, Some(&data));
        let _ = RegCloseKey(key);
        set.ok()
    }
}

/// Remove `HKCU\<subkey>` and all under it, if it is there.
pub(crate) fn delete_user_key(subkey: &str) {
    let subkey = HSTRING::from(subkey);
    let _ = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, &subkey) };
}

/// Register [`AUMID`] for the current user with a display name. Idempotent.
fn register_aumid() -> windows::core::Result<()> {
    let subkey = HSTRING::from(format!(r"Software\Classes\AppUserModelId\{AUMID}"));
    let mut key = HKEY::default();
    unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            &subkey,
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            None,
            &mut key,
            None,
        )
        .ok()?;
        let name: Vec<u8> = "Pocketdesk\0"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        let set = RegSetValueExW(key, w!("DisplayName"), None, REG_SZ, Some(&name));
        let _ = RegCloseKey(key);
        set.ok()
    }
}

/// The toast group a phone's notifications (N5a) are shown in, so a tag can
/// address one of them for removal (N5c).
const PHONE_GROUP: &str = "phone";

/// The phone toasts with buttons (N3's answers, K2b's "Not from this app"),
/// newest last, by tag: each holds its `Activated` handler, which lives only
/// as long as the toast object does.
static ANSWERABLE: std::sync::Mutex<std::collections::VecDeque<(String, ToastNotification, bool)>> =
    std::sync::Mutex::new(std::collections::VecDeque::new());

/// As many as the notes kept to answer (`phone_notes`).
const KEEP_ANSWERABLE: usize = 30;

fn answerable()
-> std::sync::MutexGuard<'static, std::collections::VecDeque<(String, ToastNotification, bool)>> {
    ANSWERABLE.lock().unwrap_or_else(|e| e.into_inner())
}

fn show_toast(
    summary: &str,
    body: &str,
    tag: Option<&str>,
    answers: Option<&crate::toast_answers::Answerable>,
    app: Option<&str>,
    pictures: &crate::toast_answers::Pictures,
) -> windows::core::Result<()> {
    // The calling thread may never have touched WinRT; an apartment that is
    // already initialized makes this a no-op error worth ignoring.
    let _ = unsafe { RoInitialize(RO_INIT_MULTITHREADED) };
    register_aumid()?;
    let doc = XmlDocument::new()?;
    // Only a tagged (phone) toast is answered or muted: its tag keeps it
    // alive.
    let answers = answers.filter(|_| tag.is_some());
    let app = app.filter(|_| tag.is_some()).map(str::to_string);
    doc.LoadXml(&HSTRING::from(crate::toast_answers::toast_xml_pictured(
        summary,
        body,
        answers,
        app.is_some(),
        pictures,
    )))?;
    let toast = ToastNotification::CreateToastNotification(&doc)?;
    if let Some(tag) = tag {
        toast.SetTag(&HSTRING::from(tag))?;
        toast.SetGroup(&HSTRING::from(PHONE_GROUP))?;
    }
    let has_buttons = answers.is_some() || app.is_some();
    if has_buttons && tag.is_some() {
        let answers = answers.cloned();
        let muting = app.clone();
        toast.Activated(&TypedEventHandler::<ToastNotification, IInspectable>::new(
            move |_, args| {
                let Some(args) = args
                    .as_ref()
                    .and_then(|args| args.cast::<ToastActivatedEventArgs>().ok())
                else {
                    return Ok(());
                };
                let arguments = args.Arguments().map(|a| a.to_string()).unwrap_or_default();
                if arguments == crate::toast_answers::MUTE {
                    if let Some(app) = &muting {
                        crate::toast_answers::mute(app);
                    }
                } else if let Some(answers) = &answers
                    && let Some(answer) = activation_answer(&args, answers, muting.is_some())
                {
                    crate::toast_answers::send(answers, answer);
                }
                Ok(())
            },
        ))?;
    }
    ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(AUMID))?.Show(&toast)?;
    // Kept only once shown, and only while the toast for this tag can be
    // answered: a plain update of it drops the old one (N3 review).
    if let Some(tag) = tag {
        let mut kept = answerable();
        kept.retain(|(t, _, _)| t != tag);
        if has_buttons {
            kept.push_back((tag.to_string(), toast, answers.is_some()));
            // Each kind its own places: thirty to answer, thirty to mute.
            crate::toast_answers::keep_within(&mut kept, |(_, _, a)| *a, KEEP_ANSWERABLE);
        }
    }
    Ok(())
}

/// What a press on an answerable toast answers: its arguments, and the reply
/// box's text when there is one.
fn activation_answer(
    args: &ToastActivatedEventArgs,
    answers: &crate::toast_answers::Answerable,
    mute: bool,
) -> Option<pocketdesk_core::local::phone_notes::Answer> {
    let arguments = args.Arguments().ok()?.to_string();
    let reply = args
        .UserInput()
        .ok()
        .and_then(|input| {
            input
                .Lookup(&HSTRING::from(crate::toast_answers::REPLY))
                .ok()
        })
        .and_then(|value| value.cast::<IPropertyValue>().ok())
        .and_then(|value| value.GetString().ok())
        .map(|text| text.to_string());
    crate::toast_answers::toast_answer(&arguments, reply.as_deref(), answers, mute)
}

/// Show a phone's notification as a toast that [`remove_tagged`] can take
/// back by `tag` (at most 64 characters), with its buttons and reply box
/// when it can be `answers`ed (N3), and "Not from this app" for `app` (K2b).
pub(super) fn notify_tagged(
    summary: &str,
    body: &str,
    tag: &str,
    answers: Option<&crate::toast_answers::Answerable>,
    app: Option<&str>,
    pictures: &crate::toast_answers::Pictures,
) -> bool {
    match show_toast(summary, body, Some(tag), answers, app, pictures) {
        Ok(()) => true,
        Err(e) => {
            tracing::debug!("toast notification failed: {e}");
            false
        }
    }
}

/// Take the phone toast tagged `tag` off the screen and out of the Action
/// Center. A tag that is not there is no error.
pub(super) fn remove_tagged(tag: &str) {
    answerable().retain(|(t, _, _)| t != tag);
    let _ = unsafe { RoInitialize(RO_INIT_MULTITHREADED) };
    let removed = ToastNotificationManager::History().and_then(|history| {
        history.RemoveGroupedTagWithId(
            &HSTRING::from(tag),
            &HSTRING::from(PHONE_GROUP),
            &HSTRING::from(AUMID),
        )
    });
    if let Err(e) = removed {
        tracing::debug!("could not remove the toast {tag}: {e}");
    }
}

pub(super) fn notify(summary: &str, body: &str) -> bool {
    match show_toast(
        summary,
        body,
        None,
        None,
        None,
        &crate::toast_answers::Pictures::default(),
    ) {
        Ok(()) => true,
        Err(e) => {
            tracing::debug!("toast notification failed: {e}");
            false
        }
    }
}

/// The event a server waits on and `pocketdesk stop` signals. Session-local,
/// so other logon sessions cannot reach it; [`listen_for_stop_event`] refuses
/// one that already exists, so a squatter cannot hand us its own.
fn stop_event_name(pid: u32) -> HSTRING {
    HSTRING::from(format!(r"Local\Pocketdesk-{pid}-stop"))
}

/// Create this process's stop event and forward its signal to `tx`.
pub(super) fn listen_for_stop_event(tx: tokio::sync::mpsc::Sender<super::ShutdownReason>) {
    let name = stop_event_name(std::process::id());
    let event = match unsafe { CreateEventW(None, true, false, &name) } {
        Ok(event) => event,
        Err(e) => {
            tracing::warn!("cannot create the stop event; `pocketdesk stop` will force-kill: {e}");
            return;
        }
    };
    // CreateEventW also "succeeds" by opening an event someone else made. One
    // that predates this process was planted, and whoever planted it controls
    // its security and can fire it: do not listen to it.
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        let _ = unsafe { CloseHandle(event) };
        tracing::warn!(
            "a stop event for this pid already existed (planted?); ignoring it, so              `pocketdesk stop` will fall back to terminating the process"
        );
        return;
    }
    // HANDLE is a raw pointer; the value crosses to the waiting thread.
    let raw = event.0 as isize;
    let spawned = std::thread::Builder::new()
        .name("stop-event".into())
        .spawn(move || {
            let event = windows::Win32::Foundation::HANDLE(raw as *mut _);
            unsafe { WaitForSingleObject(event, INFINITE) };
            let _ = tx.blocking_send("pocketdesk stop");
            let _ = unsafe { CloseHandle(event) };
        });
    if let Err(e) = spawned {
        tracing::warn!("cannot wait on the stop event: {e}");
    }
}

/// Signal the stop event of the server running as `pid`; `false` when there
/// is none (an older or wedged server).
pub(super) fn signal_stop_event(pid: u32) -> bool {
    let Ok(event) = (unsafe { OpenEventW(EVENT_MODIFY_STATE, false, &stop_event_name(pid)) })
    else {
        return false;
    };
    let set = unsafe { SetEvent(event) }.is_ok();
    let _ = unsafe { CloseHandle(event) };
    set
}

/// Whether two executable paths name the same program file (Windows paths
/// compare case-insensitively). `pocketdesk` and `pd` are one program.
fn same_program(a: &std::path::Path, b: &std::path::Path) -> bool {
    use pocketdesk_core::local::server::is_server_image;
    match (a.file_name(), b.file_name()) {
        (Some(a), Some(b)) => {
            let (a, b) = (a.to_string_lossy(), b.to_string_lossy());
            a.eq_ignore_ascii_case(&b) || (is_server_image(&a) && is_server_image(&b))
        }
        _ => false,
    }
}

/// Terminate `pid`, but only if it is running this same program: a PID file
/// left behind by a crash may by now name an unrelated process.
pub(super) fn force_stop(pid: u32) -> std::io::Result<bool> {
    let os_err = |e: windows::core::Error| std::io::Error::from_raw_os_error(e.code().0 & 0xFFFF);
    let process = unsafe {
        OpenProcess(
            PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION,
            false,
            pid,
        )
    }
    .map_err(os_err)?;
    let mut buffer = [0u16; 1024];
    let mut len = buffer.len() as u32;
    let named = unsafe {
        QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            windows::core::PWSTR(buffer.as_mut_ptr()),
            &mut len,
        )
    };
    let image = std::path::PathBuf::from(String::from_utf16_lossy(&buffer[..len as usize]));
    let ours = std::env::current_exe()?;
    let result = if named.is_ok() && same_program(&image, &ours) {
        unsafe { TerminateProcess(process, 1) }
            .map(|()| true)
            .map_err(os_err)
    } else {
        tracing::warn!(
            pid,
            image = %image.display(),
            "not terminating: that pid is not a Pocketdesk server"
        );
        Ok(false)
    };
    let _ = unsafe { CloseHandle(process) };
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::UI::HiDpi::{AreDpiAwarenessContextsEqual, GetThreadDpiAwarenessContext};

    #[test]
    fn the_process_holds_the_multithreaded_apartment() {
        init_process();
        // A thread that never entered COM is in the apartment the process
        // holds (the implicit MTA), not outside COM.
        let (kind, qualifier) = std::thread::spawn(|| {
            let mut kind = windows::Win32::System::Com::APTTYPE::default();
            let mut qualifier = windows::Win32::System::Com::APTTYPEQUALIFIER::default();
            unsafe { windows::Win32::System::Com::CoGetApartmentType(&mut kind, &mut qualifier) }
                .expect("in an apartment");
            (kind, qualifier)
        })
        .join()
        .unwrap();
        assert_eq!(kind, windows::Win32::System::Com::APTTYPE_MTA);
        assert_eq!(
            qualifier,
            windows::Win32::System::Com::APTTYPEQUALIFIER_IMPLICIT_MTA
        );
    }

    #[test]
    fn a_fault_is_described_with_its_module_and_what_is_there() {
        use windows::Win32::Foundation::NTSTATUS;
        use windows::Win32::System::Diagnostics::Debug::EXCEPTION_RECORD;
        remember_modules();
        let here = a_fault_is_described_with_its_module_and_what_is_there as *const () as usize;
        let mut record = EXCEPTION_RECORD {
            ExceptionCode: NTSTATUS(0xC000_0005_u32 as i32),
            ExceptionAddress: here as *mut _,
            NumberParameters: 2,
            ..Default::default()
        };
        record.ExceptionInformation[0] = 8;
        record.ExceptionInformation[1] = here;
        let mut out = Vec::new();
        describe(&mut out, &record);
        let text = String::from_utf8(out).unwrap();
        assert!(text.starts_with("exception 0xc0000005 at "), "{text}");
        assert!(
            text.contains(".exe+0x"),
            "named by this test's module: {text}"
        );
        assert!(text.contains("(image)"), "{text}");
        assert!(text.contains("(executing "), "{text}");
    }

    #[test]
    fn memory_given_back_reads_as_unmapped() {
        use windows::Win32::System::Memory::{
            MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE, VirtualAlloc, VirtualFree,
        };
        // SAFETY: one page of this process's own, given back below.
        let page = unsafe { VirtualAlloc(None, 4096, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE) };
        assert!(!page.is_null());
        assert_eq!(mapping(page as usize), "not an image");
        // SAFETY: the page just allocated, released whole.
        unsafe { VirtualFree(page, 0, MEM_RELEASE) }.unwrap();
        assert_eq!(mapping(page as usize), "unmapped");
    }

    /// The phone toasts in the Action Center, by tag.
    fn phone_tags_in_history() -> Vec<String> {
        let history = ToastNotificationManager::History()
            .and_then(|h| h.GetHistoryWithId(&HSTRING::from(AUMID)))
            .expect("toast history");
        (0..history.Size().unwrap())
            .filter_map(|i| history.GetAt(i).ok())
            .filter(|t| t.Group().map(|g| g == PHONE_GROUP).unwrap_or(false))
            .filter_map(|t| t.Tag().ok().map(|tag| tag.to_string()))
            .collect()
    }

    /// Live (N5c): a tagged phone toast is in the Action Center until its tag
    /// is removed. Flashes one toast.
    /// `cargo test -p pocketdesk-server --lib a_tagged_toast -- --ignored`.
    #[test]
    #[ignore = "shows a real toast on an interactive Windows desktop"]
    fn a_tagged_toast_can_be_taken_back() {
        let tag = "0123456789abcdef";
        assert!(notify_tagged(
            "Pocketdesk test",
            "This toast removes itself",
            tag,
            None,
            None,
            &crate::toast_answers::Pictures::default()
        ));
        std::thread::sleep(std::time::Duration::from_millis(500));
        assert!(phone_tags_in_history().iter().any(|t| t == tag), "shown");
        remove_tagged(tag);
        std::thread::sleep(std::time::Duration::from_millis(500));
        assert!(
            !phone_tags_in_history().iter().any(|t| t == tag),
            "taken back"
        );
        remove_tagged("not-there");
    }

    #[test]
    fn the_process_ends_up_per_monitor_v2_aware() {
        init_process();
        let current = unsafe { GetThreadDpiAwarenessContext() };
        assert!(
            unsafe {
                AreDpiAwarenessContextsEqual(current, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)
            }
            .as_bool()
        );
    }

    /// Signals only this test process's own event, but it exercises the
    /// server-shutdown path, so it stays opt-in.
    #[tokio::test]
    #[ignore = "exercises the shutdown path; opt-in only"]
    async fn the_stop_event_reaches_the_shutdown_channel() {
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        listen_for_stop_event(tx);
        assert!(signal_stop_event(std::process::id()));
        let reason = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .expect("stop request delivered");
        assert_eq!(reason, Some("pocketdesk stop"));
        assert!(
            !signal_stop_event(u32::MAX),
            "no event for a pid that is not running"
        );
    }

    #[test]
    fn only_the_same_program_counts_as_a_server() {
        use std::path::Path;
        assert!(same_program(
            Path::new(r"C:\Program Files\Pocketdesk\pocketdesk.exe"),
            Path::new(r"E:\build\POCKETDESK.EXE")
        ));
        assert!(
            same_program(
                Path::new(r"C:\Program Files\Pocketdesk\pocketdesk.exe"),
                Path::new(r"E:\build\pd.exe")
            ),
            "the short name is the same program"
        );
        assert!(
            !same_program(
                Path::new(r"C:\Program Files\Pocketdesk\pocketdesk.exe"),
                Path::new(r"E:\build\pocketdesk-app.exe")
            ),
            "the desktop app is not the server"
        );
        assert!(!same_program(
            Path::new(r"C:\Windows\explorer.exe"),
            Path::new(r"E:\build\pocketdesk.exe")
        ));
        assert!(!same_program(
            Path::new(""),
            Path::new(r"E:\pocketdesk.exe")
        ));
    }

    #[test]
    fn toast_text_is_escaped_and_loads_as_xml() {
        let xml = crate::toast_answers::toast_xml("PIN <1234> & go", "a > b", None, false);
        assert!(xml.contains("PIN &lt;1234&gt; &amp; go"));
        let doc = XmlDocument::new().unwrap();
        doc.LoadXml(&HSTRING::from(xml))
            .expect("well-formed toast XML");
    }

    fn answerable() -> crate::toast_answers::Answerable {
        crate::toast_answers::Answerable {
            device_id: "phone".into(),
            key: "0|chat|1|null|10001".into(),
            actions: vec!["Mark \"read\" & <go>".into(), "Mute".into()],
            reply: true,
        }
    }

    /// N3: Windows takes the reply box and buttons as a toast.
    #[test]
    fn an_answerable_toast_loads_as_toast_xml() {
        let xml = crate::toast_answers::toast_xml("Chat: Ann", "hi", Some(&answerable()), false);
        let doc = XmlDocument::new().unwrap();
        doc.LoadXml(&HSTRING::from(xml.clone()))
            .expect("well-formed");
        ToastNotification::CreateToastNotification(&doc).expect("a toast Windows accepts");
    }

    /// K3: Windows takes a toast with its app's logo and a picture.
    #[test]
    fn a_pictured_toast_loads_as_toast_xml() {
        let pictures = crate::toast_answers::Pictures {
            icon: Some(std::path::PathBuf::from(
                r"C:\state\notification_icons\ab.png",
            )),
            picture: Some(std::path::PathBuf::from(
                r"C:\state\notification_icons\pictures\c d.jpg",
            )),
        };
        let xml = crate::toast_answers::toast_xml_pictured(
            "Chat: Ann",
            "hi",
            Some(&answerable()),
            true,
            &pictures,
        );
        let doc = XmlDocument::new().unwrap();
        doc.LoadXml(&HSTRING::from(xml)).expect("well-formed");
        ToastNotification::CreateToastNotification(&doc).expect("a toast Windows accepts");
    }

    /// Live (N3): an answerable phone toast is shown, with its reply box and
    /// buttons, and taken back. Flashes one toast; pressing it is the owner's.
    /// `cargo test -p pocketdesk-server --lib an_answerable_toast_is_shown -- --ignored`.
    #[test]
    #[ignore = "shows a real toast on an interactive Windows desktop"]
    fn an_answerable_toast_is_shown() {
        let tag = "fedcba9876543210";
        assert!(notify_tagged(
            "Pocketdesk test: Chat",
            "Reply or press a button",
            tag,
            Some(&answerable()),
            None,
            &crate::toast_answers::Pictures::default()
        ));
        std::thread::sleep(std::time::Duration::from_millis(500));
        assert!(phone_tags_in_history().iter().any(|t| t == tag), "shown");
        assert!(
            super::answerable().iter().any(|(t, _, _)| t == tag),
            "kept to answer"
        );
        remove_tagged(tag);
        assert!(
            !super::answerable().iter().any(|(t, _, _)| t == tag),
            "dropped with its toast"
        );
    }

    /// Live: a toast actually reaches the Action Center. Run on a desktop:
    /// `cargo test -p pocketdesk-server --lib a_toast_is_shown -- --ignored`.
    #[test]
    #[ignore = "shows a real toast on an interactive Windows desktop"]
    fn a_toast_is_shown() {
        assert!(notify(
            "Pocketdesk test",
            "If you can read this, toasts work."
        ));
    }
}
