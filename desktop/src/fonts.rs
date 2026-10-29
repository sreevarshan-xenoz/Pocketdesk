//! Glyphs for every script (U1). egui ships fonts for Latin and a few more,
//! so Tamil and the other Indic scripts, CJK, Arabic and Thai in a phone's
//! notifications, texts, names and file names showed as empty boxes. The
//! system's fonts for them are added after egui's own, so its look stays and
//! they fill only what it lacks. Each is mapped, not read: only the pages a
//! drawn glyph needs are loaded.

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use eframe::egui;

/// The fonts added, by name, for Diagnostics.
static LOADED: OnceLock<Vec<String>> = OnceLock::new();

/// The fonts added at start, once looked for (`None` while still looking).
pub fn loaded() -> Option<&'static [String]> {
    LOADED.get().map(Vec::as_slice)
}

/// Diagnostics' line about the fonts for other scripts; none while they
/// are still looked for (UX 1: a stray line before any check ran).
pub fn summary(loaded: Option<&[String]>) -> Option<String> {
    match loaded? {
        [] => Some(
            "Fonts for other scripts: none found, so some names and texts may show as boxes."
                .to_string(),
        ),
        names => Some(format!("Fonts for other scripts: {}.", names.join(", "))),
    }
}

/// A font to look for: its name and its file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub name: String,
    pub path: PathBuf,
}

/// Windows' fonts for other scripts, in order: Indic, then Arabic, Hebrew
/// and more, Thai, Chinese (with kana), Japanese, Korean.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn windows_candidates(fonts_dir: &Path) -> Vec<Candidate> {
    [
        ("Nirmala UI", "Nirmala.ttc"),
        ("Nirmala UI", "Nirmala.ttf"),
        ("Segoe UI", "segoeui.ttf"),
        ("Leelawadee UI", "LeelawUI.ttf"),
        ("Microsoft YaHei", "msyh.ttc"),
        ("Yu Gothic", "YuGothR.ttc"),
        ("Malgun Gothic", "malgun.ttf"),
    ]
    .into_iter()
    .map(|(name, file)| Candidate {
        name: name.to_string(),
        path: fonts_dir.join(file),
    })
    .collect()
}

/// The Noto families to look for on Linux, in order.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const NOTO_FAMILIES: [&str; 6] = [
    "Noto Sans Tamil",
    "Noto Sans Devanagari",
    "Noto Sans Arabic",
    "Noto Sans Thai",
    "Noto Sans CJK SC",
    "Noto Sans",
];

/// The Noto fonts in `fc-list : family file` output, in [`NOTO_FAMILIES`]'s
/// order: each family's first regular file.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn noto_from_fc_list(listing: &str) -> Vec<Candidate> {
    let mut found = Vec::new();
    for family in NOTO_FAMILIES {
        let regular = listing.lines().find_map(|line| {
            // "/usr/share/fonts/…/NotoSansTamil-Regular.ttf: Noto Sans Tamil"
            let (file, families) = line.split_once(": ")?;
            let named = families.split(',').any(|f| f.trim() == family);
            // A static family's regular file, or a variable font (one file,
            // no "-Style" in its name; its folder may well have a "-").
            let name = Path::new(file.trim())
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("");
            let regular = name.contains("Regular") || !name.contains('-');
            (named && regular).then(|| file.trim().to_string())
        });
        if let Some(file) = regular {
            found.push(Candidate {
                name: family.to_string(),
                path: PathBuf::from(file),
            });
        }
    }
    found
}

/// This system's candidates.
fn candidates() -> Vec<Candidate> {
    #[cfg(windows)]
    {
        let windir = std::env::var_os("WINDIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
        windows_candidates(&windir.join("Fonts"))
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("fc-list")
            .args([":", "family", "file"])
            .output()
            .ok()
            .map(|out| noto_from_fc_list(&String::from_utf8_lossy(&out.stdout)))
            .unwrap_or_default()
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        Vec::new()
    }
}

/// The candidates whose files exist, one per name (the first found).
pub fn present(candidates: Vec<Candidate>, exists: impl Fn(&Path) -> bool) -> Vec<Candidate> {
    let mut kept: Vec<Candidate> = Vec::new();
    for candidate in candidates {
        if exists(&candidate.path) && !kept.iter().any(|k| k.name == candidate.name) {
            kept.push(candidate);
        }
    }
    kept
}

/// Add the system's fonts for other scripts after egui's own, found on a
/// thread of their own (`fc-list` and the font files are not the UI's to
/// wait on); the app repaints with them once they are in.
pub fn install(ctx: &egui::Context) {
    let ctx = ctx.clone();
    let spawned = std::thread::Builder::new()
        .name("fonts".into())
        .spawn(move || {
            if let Some(definitions) = definitions() {
                ctx.set_fonts(definitions);
                ctx.request_repaint();
            }
        });
    if spawned.is_err() {
        let _ = LOADED.set(Vec::new());
    }
}

/// egui's fonts with the system's for other scripts after them; `None`
/// when none was found.
fn definitions() -> Option<egui::FontDefinitions> {
    let mut definitions = egui::FontDefinitions::default();
    let mut names = Vec::new();
    for candidate in present(candidates(), Path::exists) {
        let Ok(file) = std::fs::File::open(&candidate.path) else {
            continue;
        };
        // SAFETY: a system font file, not written while the app runs; the
        // map is kept for the app's life.
        let Ok(map) = (unsafe { memmap2::Mmap::map(&file) }) else {
            continue;
        };
        // As epaint parses it: a file that does not parse is left out, not
        // a panic at the first frame (U1 review).
        if skrifa::FontRef::from_index(&map[..], 0).is_err() {
            continue;
        }
        let map: &'static memmap2::Mmap = Box::leak(Box::new(map));
        let bytes: &'static [u8] = &map[..];
        let key = format!("system: {}", candidate.name);
        definitions
            .font_data
            .insert(key.clone(), Arc::new(egui::FontData::from_static(bytes)));
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            definitions
                .families
                .entry(family)
                .or_default()
                .push(key.clone());
        }
        names.push(candidate.name);
    }
    let found = !names.is_empty();
    let _ = LOADED.set(names);
    found.then_some(definitions)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_summary_tells_looking_from_none() {
        assert_eq!(summary(None), None, "nothing to say while looking");
        assert!(summary(Some(&[])).is_some_and(|s| s.contains("none found")));
        assert_eq!(
            summary(Some(&[
                "Nirmala UI".to_string(),
                "Leelawadee UI".to_string()
            ]))
            .as_deref(),
            Some("Fonts for other scripts: Nirmala UI, Leelawadee UI.")
        );
    }

    #[test]
    fn missing_fonts_are_skipped_and_a_name_found_once_is_kept_once() {
        let dir = Path::new(r"C:\Windows\Fonts");
        let all = windows_candidates(dir);
        // Windows 10 has Nirmala.ttf, Windows 11 Nirmala.ttc.
        let present = present(all, |p| {
            !p.ends_with("Nirmala.ttc") && !p.ends_with("malgun.ttf")
        });
        let names: Vec<_> = present.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "Nirmala UI",
                "Segoe UI",
                "Leelawadee UI",
                "Microsoft YaHei",
                "Yu Gothic"
            ]
        );
        assert!(present[0].path.ends_with("Nirmala.ttf"));
    }

    #[test]
    fn a_file_that_is_not_a_font_is_left_out() {
        assert!(skrifa::FontRef::from_index(b"not a font at all", 0).is_err());
    }

    #[test]
    fn noto_fonts_are_found_in_fc_lists_output() {
        let listing = "\
/usr/share/fonts/noto/NotoSansTamil-Bold.ttf: Noto Sans Tamil
/usr/share/fonts/noto/NotoSansTamil-Regular.ttf: Noto Sans Tamil
/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc: Noto Sans CJK JP,Noto Sans CJK SC,Noto Sans CJK TC
/usr/share/fonts/dejavu/DejaVuSans.ttf: DejaVu Sans
/usr/share/fonts/google-noto-vf/NotoSansThai[wdth,wght].ttf: Noto Sans Thai
";
        let found = noto_from_fc_list(listing);
        let names: Vec<_> = found.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(
            names,
            ["Noto Sans Tamil", "Noto Sans Thai", "Noto Sans CJK SC"]
        );
        assert!(found[0].path.ends_with("NotoSansTamil-Regular.ttf"));
    }
}

#[cfg(test)]
mod probe {
    use super::*;

    #[test]
    #[ignore = "reads this system's fonts"]
    fn the_systems_fonts_draw_other_scripts() {
        let ctx = egui::Context::default();
        ctx.set_fonts(definitions().expect("fonts found"));
        let mut out = ctx.run_ui(egui::RawInput::default(), |_| {});
        out.textures_delta.clear();
        eprintln!("loaded: {:?}", loaded());
        let id = egui::FontId::proportional(14.0);
        for c in ['த', '中', 'ع', 'ไ', '한'] {
            let has = ctx.fonts_mut(|f| f.has_glyph(&id, c));
            eprintln!("{c}: {has}");
        }
    }
}
