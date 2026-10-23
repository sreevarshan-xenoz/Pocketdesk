//! Quiet hours (P3): from a time to a time on chosen days, this computer
//! holds phones' notifications as a pause does (K2a): logged and kept to
//! answer, no toast. A span past midnight belongs to the day it starts.

use super::config::Config;

/// The days of the week as the config names them, Monday first.
pub const DAYS: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];

/// A schedule: minutes after midnight, and the days it starts on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Schedule {
    pub from: u16,
    pub to: u16,
    /// Bit 0 is Monday.
    pub days: u8,
}

/// The local time: the day of the week (0 is Monday) and minutes after
/// midnight.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalTime {
    pub weekday: u8,
    pub minute: u16,
}

/// `"22:00"` as minutes after midnight; `None` for anything else.
pub fn parse_time(text: &str) -> Option<u16> {
    let (h, m) = text.trim().split_once(':')?;
    if h.is_empty() || h.len() > 2 || m.len() != 2 {
        return None;
    }
    let (h, m): (u16, u16) = (h.parse().ok()?, m.parse().ok()?);
    (h < 24 && m < 60).then_some(h * 60 + m)
}

/// The days named, as bits; unknown names are left out.
pub fn days_mask(days: &[String]) -> u8 {
    days.iter()
        .filter_map(|d| DAYS.iter().position(|n| n.eq_ignore_ascii_case(d.trim())))
        .fold(0, |mask, i| mask | 1 << i)
}

impl Schedule {
    /// The config's schedule, when quiet hours are on and it can be read.
    pub fn from_config(config: &Config) -> Option<Self> {
        if !config.quiet_hours {
            return None;
        }
        Some(Schedule {
            from: parse_time(&config.quiet_from)?,
            to: parse_time(&config.quiet_to)?,
            days: days_mask(&config.quiet_days),
        })
    }

    fn starts_on(&self, weekday: u8) -> bool {
        self.days & (1 << (weekday % 7)) != 0
    }

    /// Whether `at` is within quiet hours. A span past midnight is the day
    /// it starts on; a span from a time to the same time is the whole day.
    pub fn holds(&self, at: LocalTime) -> bool {
        let yesterday = (at.weekday + 6) % 7;
        match self.from.cmp(&self.to) {
            std::cmp::Ordering::Less => {
                self.starts_on(at.weekday) && (self.from..self.to).contains(&at.minute)
            }
            std::cmp::Ordering::Greater => {
                (self.starts_on(at.weekday) && at.minute >= self.from)
                    || (self.starts_on(yesterday) && at.minute < self.to)
            }
            std::cmp::Ordering::Equal => self.starts_on(at.weekday),
        }
    }
}

/// This computer's local time now; `None` where it cannot be read.
pub fn local_now() -> Option<LocalTime> {
    #[cfg(windows)]
    {
        // SAFETY: GetLocalTime takes nothing and returns the time.
        let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
        // wDayOfWeek counts from Sunday.
        Some(LocalTime {
            weekday: ((t.wDayOfWeek + 6) % 7) as u8,
            minute: t.wHour * 60 + t.wMinute,
        })
    }
    #[cfg(unix)]
    {
        // SAFETY: `time` with a null pointer only returns; `localtime_r`
        // fills the struct it is given and returns null on failure.
        unsafe {
            let now = libc::time(std::ptr::null_mut());
            let mut tm: libc::tm = std::mem::zeroed();
            if libc::localtime_r(&now, &mut tm).is_null() {
                return None;
            }
            Some(LocalTime {
                weekday: ((tm.tm_wday + 6) % 7) as u8,
                minute: (tm.tm_hour * 60 + tm.tm_min) as u16,
            })
        }
    }
    #[cfg(not(any(windows, unix)))]
    {
        None
    }
}

/// Whether Windows says this computer is presenting, or runs a full-screen
/// app or game: its own "do not disturb" for notifications. A test seam,
/// `POCKETDESK_PRESENTING_FILE`, answers by whether that file exists.
pub fn presenting() -> bool {
    presenting_with(std::env::var_os("POCKETDESK_PRESENTING_FILE"))
}

/// [`presenting`], with the seam's file (P3 review: tested without
/// changing the process's environment).
pub fn presenting_with(seam: Option<std::ffi::OsString>) -> bool {
    if let Some(path) = seam {
        return std::path::Path::new(&path).exists();
    }
    #[cfg(windows)]
    {
        use windows::Win32::UI::Shell::{
            QUNS_BUSY, QUNS_PRESENTATION_MODE, QUNS_RUNNING_D3D_FULL_SCREEN,
            SHQueryUserNotificationState,
        };
        // SAFETY: a query with no arguments but its answer.
        unsafe { SHQueryUserNotificationState() }.is_ok_and(|state| {
            [
                QUNS_BUSY,
                QUNS_RUNNING_D3D_FULL_SCREEN,
                QUNS_PRESENTATION_MODE,
            ]
            .contains(&state)
        })
    }
    #[cfg(target_os = "linux")]
    {
        linux::do_not_disturb()
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        false
    }
}

/// Q4: the desktop's own Do Not Disturb on Linux, as GNOME and KDE say it.
/// A thread of its own asks every 5 s while someone asks (R2: the server
/// and the desktop app's Overview), each
/// with a second's deadline, and the handlers only read its last answer
/// (Q4 review: asked on the runtime's threads under a lock, a hung
/// `gsettings` held every notification behind it). A desktop that says
/// nothing is not quiet; the first answer comes within a second of the
/// first ask.
#[cfg(target_os = "linux")]
mod linux {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    static QUIET: AtomicBool = AtomicBool::new(false);
    static ASKING: AtomicBool = AtomicBool::new(false);

    /// When it was last asked, in seconds since the thread's epoch.
    static ASKED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    static EPOCH: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

    fn now_secs() -> u64 {
        EPOCH.get_or_init(Instant::now).elapsed().as_secs()
    }

    pub fn do_not_disturb() -> bool {
        ASKED.store(now_secs(), Ordering::Release);
        if !ASKING.swap(true, Ordering::AcqRel) {
            let _ = std::thread::Builder::new()
                .name("do-not-disturb".into())
                .spawn(|| {
                    loop {
                        let quiet = gnome() == Some(true) || kde() == Some(true);
                        QUIET.store(quiet, Ordering::Release);
                        std::thread::sleep(Duration::from_secs(5));
                        // Ends a minute after the last ask (the switch off,
                        // or no one asking); the next ask starts it again.
                        if now_secs().saturating_sub(ASKED.load(Ordering::Acquire)) > 60 {
                            QUIET.store(false, Ordering::Release);
                            ASKING.store(false, Ordering::Release);
                            break;
                        }
                    }
                })
                .inspect_err(|_| ASKING.store(false, Ordering::Release));
        }
        QUIET.load(Ordering::Acquire)
    }

    /// GNOME: banners off is its Do Not Disturb.
    fn gnome() -> Option<bool> {
        let out = output_within(
            "gsettings",
            &["get", "org.gnome.desktop.notifications", "show-banners"],
        )?;
        super::gnome_quiet(&out)
    }

    /// KDE: its notification server says it is inhibited.
    fn kde() -> Option<bool> {
        let out = output_within(
            "gdbus",
            &[
                "call",
                "--session",
                "--dest",
                "org.freedesktop.Notifications",
                "--object-path",
                "/org/freedesktop/Notifications",
                "--method",
                "org.freedesktop.DBus.Properties.Get",
                "org.freedesktop.Notifications",
                "Inhibited",
            ],
        )?;
        super::inhibited(&out)
    }

    /// A command's output, or none when it fails or takes over a second.
    fn output_within(program: &str, args: &[&str]) -> Option<String> {
        use std::io::Read;
        let mut child = std::process::Command::new(program)
            .args(args)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .ok()?;
        let mut stdout = child.stdout.take()?;
        let reader = std::thread::spawn(move || {
            let mut out = String::new();
            let _ = stdout.read_to_string(&mut out);
            out
        });
        let end = Instant::now() + Duration::from_secs(1);
        loop {
            match child.try_wait() {
                Ok(Some(status)) if status.success() => return reader.join().ok(),
                Ok(Some(_)) | Err(_) => return None,
                Ok(None) if Instant::now() >= end => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            }
        }
    }
}

/// Q4: GNOME's `show-banners` answer read: `false` is Do Not Disturb.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn gnome_quiet(out: &str) -> Option<bool> {
    match out.trim() {
        "false" => Some(true),
        "true" => Some(false),
        _ => None,
    }
}

/// Q4: `gdbus`'s answer for KDE's `Inhibited`, such as `(<true>,)`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn inhibited(out: &str) -> Option<bool> {
    let out = out.trim();
    if out.contains("<true>") {
        Some(true)
    } else if out.contains("<false>") {
        Some(false)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Q4: GNOME's and KDE's answers read; anything else says nothing.
    #[test]
    fn the_desktops_do_not_disturb_is_read() {
        assert_eq!(gnome_quiet("false\n"), Some(true));
        assert_eq!(gnome_quiet("true\n"), Some(false));
        assert_eq!(gnome_quiet("No such schema"), None);
        assert_eq!(inhibited("(<true>,)\n"), Some(true));
        assert_eq!(inhibited("(<false>,)"), Some(false));
        assert_eq!(inhibited("Error: GDBus.Error"), None);
    }

    fn at(weekday: u8, time: &str) -> LocalTime {
        LocalTime {
            weekday,
            minute: parse_time(time).unwrap(),
        }
    }

    fn every_day(from: &str, to: &str) -> Schedule {
        Schedule {
            from: parse_time(from).unwrap(),
            to: parse_time(to).unwrap(),
            days: 0x7f,
        }
    }

    #[test]
    fn times_are_read_as_written_and_nothing_else() {
        assert_eq!(parse_time("22:00"), Some(22 * 60));
        assert_eq!(parse_time("7:05"), Some(7 * 60 + 5));
        assert_eq!(parse_time(" 00:00 "), Some(0));
        for bad in ["24:00", "12:60", "12", "12:5", "ab:cd", "", ":30", "123:00"] {
            assert_eq!(parse_time(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn days_are_read_by_name() {
        let days = |d: &[&str]| days_mask(&d.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(days(&["mon"]), 1);
        assert_eq!(days(&["Sun", "sat"]), 0b110_0000);
        assert_eq!(days(&DAYS), 0x7f);
        assert_eq!(days(&["funday"]), 0, "an unknown day is no day");
    }

    #[test]
    fn a_span_within_a_day_holds_from_its_start_until_its_end() {
        let s = every_day("09:00", "17:00");
        assert!(!s.holds(at(0, "08:59")));
        assert!(s.holds(at(0, "09:00")));
        assert!(s.holds(at(0, "16:59")));
        assert!(!s.holds(at(0, "17:00")), "the end is not quiet");
    }

    #[test]
    fn a_span_past_midnight_belongs_to_the_day_it_starts() {
        // Weeknights: Monday to Friday, 22:00 to 07:00.
        let s = Schedule {
            days: 0b1_1111,
            ..every_day("22:00", "07:00")
        };
        assert!(s.holds(at(0, "23:00")), "Monday night");
        assert!(s.holds(at(1, "06:59")), "Tuesday morning, Monday's night");
        assert!(!s.holds(at(1, "07:00")));
        assert!(s.holds(at(4, "22:30")), "Friday night");
        assert!(s.holds(at(5, "06:00")), "Saturday morning, Friday's night");
        assert!(!s.holds(at(5, "23:00")), "Saturday night is not chosen");
        assert!(!s.holds(at(0, "06:00")), "Monday morning is Sunday's night");
    }

    #[test]
    fn the_same_time_twice_is_the_whole_day_and_no_day_is_never() {
        let s = Schedule {
            days: 1,
            ..every_day("00:00", "00:00")
        };
        assert!(s.holds(at(0, "00:00")) && s.holds(at(0, "23:59")));
        assert!(!s.holds(at(1, "12:00")));
        let none = Schedule {
            days: 0,
            ..every_day("22:00", "07:00")
        };
        assert!(!none.holds(at(0, "23:00")) && !none.holds(at(1, "01:00")));
    }

    #[test]
    fn the_schedule_comes_from_the_config_only_when_on_and_readable() {
        let mut config = Config::default();
        assert_eq!(Schedule::from_config(&config), None, "off by default");
        config.quiet_hours = true;
        assert_eq!(
            Schedule::from_config(&config),
            Some(every_day("22:00", "07:00")),
            "every night by default"
        );
        config.quiet_to = "7am".into();
        assert_eq!(
            Schedule::from_config(&config),
            None,
            "unreadable: not quiet"
        );
    }

    #[test]
    fn the_local_time_reads() {
        let now = local_now().expect("this computer's time");
        assert!(now.weekday < 7 && now.minute < 24 * 60);
    }
}
