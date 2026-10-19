//! The phone's recent photos on the Phone page (C7): the newest camera
//! photos as thumbnails, with Save, Open and Copy. Refresh and each action
//! are requests the server hands to the phone; a photo is fetched only when
//! asked, through the usual transfer, into the received files, where Open
//! and Copy find it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use eframe::egui::{self, RichText};
use pocketdesk_core::local::phone_photos::{self as store, Ask, Kept};
use pocketdesk_core::local::received::{self, ReceivedFile};

const REFRESH: Duration = Duration::from_secs(1);

/// How long a list waits for the phone's answer before the page says so.
const ANSWER_SECS: u64 = 15;

/// How long a fetched photo is waited for before the page says so.
const FETCH_SECS: u64 = 120;

/// A thumbnail's side on the page, in points.
const TILE: f32 = 120.0;

/// What to do with a photo once it is here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Then {
    Save,
    Open,
    Copy,
}

/// A photo asked for and not here yet.
struct Pending {
    device: String,
    name: String,
    /// Its size as the phone said, which tells two photos of one name apart.
    size: u64,
    then: Then,
    asked_unix: u64,
}

#[derive(Default)]
pub struct PhotosView {
    kept: std::collections::BTreeMap<String, Kept>,
    read_at: Option<Instant>,
    /// Thumbnails drawn, by phone, photo and the answer's time.
    textures: HashMap<(String, u64, u64), Option<egui::TextureHandle>>,
    /// The phone and the time of the list request still waiting.
    waiting: Option<(String, u64)>,
    pending: Vec<Pending>,
    /// A full image being decoded for Copy.
    copying: Option<mpsc::Receiver<Result<egui::ColorImage, String>>>,
    /// What it said happened, until the app takes it (E5).
    notice: Option<crate::notice::Notice>,
}

/// Where the photo `name` of `size` bytes (0 unknown) from `device`, asked
/// for at `since_unix`, was saved, once it is among the received files
/// (saved as `name`, or as `stem (n).ext` beside another of that name; of
/// that size, so two photos of one name are told apart).
pub fn arrived(
    rows: &[ReceivedFile],
    device: &str,
    name: &str,
    size: u64,
    since_unix: u64,
) -> Option<PathBuf> {
    let (stem, ext) = match name.rfind('.') {
        Some(dot) if dot > 0 => (&name[..dot], &name[dot..]),
        _ => (name, ""),
    };
    rows.iter()
        .rev()
        .filter(|r| {
            r.from == device && r.received_unix + 1 >= since_unix && (size == 0 || r.size == size)
        })
        .find(|r| {
            r.name == name
                || r.name
                    .strip_prefix(stem)
                    .and_then(|rest| rest.strip_suffix(ext))
                    .and_then(|n| n.strip_prefix(" (").and_then(|n| n.strip_suffix(')')))
                    .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
        })
        .map(|r| PathBuf::from(&r.path))
}

/// Whether a file named `name` may be opened with the system's viewer: a
/// picture only. The name is the phone's; nothing else is opened from it.
pub fn openable(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    [
        ".jpg", ".jpeg", ".png", ".webp", ".heic", ".heif", ".gif", ".bmp",
    ]
    .iter()
    .any(|ext| lower.ends_with(ext))
}

/// How long ago a photo taken at `taken_ms` (0 unknown) was, in words.
fn taken_text(taken_ms: u64, now_unix: u64) -> String {
    if taken_ms == 0 {
        String::new()
    } else {
        crate::words::ago(now_unix, taken_ms / 1000)
    }
}

/// Open `path` with the system's viewer for it.
fn open_file(path: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        // Explorer opens a file with its default program.
        std::process::Command::new(crate::reveal::windows_dir().join("explorer.exe"))
            .arg(path)
            .spawn()
            .map(drop)
    }
    #[cfg(target_os = "linux")]
    {
        let mut child = std::process::Command::new("xdg-open").arg(path).spawn()?;
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(())
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = path;
        Err(std::io::Error::other("not supported on this system"))
    }
}

/// The longest side a copied photo keeps: a phone's 50 or 108 MP photo
/// would hold gigabytes on its way to the clipboard (C7 review).
const MAX_COPY_SIDE: u32 = 4096;

/// Whether a photo named `name` can be copied: a format the app decodes.
pub fn copyable(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    [".jpg", ".jpeg", ".png", ".webp"]
        .iter()
        .any(|ext| lower.ends_with(ext))
}

/// The full image at `path` for the clipboard, decoded off the UI thread,
/// turned as the camera's orientation says (a portrait photo is stored on
/// its side) and at most [`MAX_COPY_SIDE`] a side.
fn decode(path: PathBuf) -> mpsc::Receiver<Result<egui::ColorImage, String>> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let read = || -> image::ImageResult<egui::ColorImage> {
            use image::ImageDecoder;
            let mut decoder = image::ImageReader::open(&path)?
                .with_guessed_format()?
                .into_decoder()?;
            let orientation = decoder.orientation()?;
            let mut img = image::DynamicImage::from_decoder(decoder)?;
            img.apply_orientation(orientation);
            if img.width().max(img.height()) > MAX_COPY_SIDE {
                img = img.resize(
                    MAX_COPY_SIDE,
                    MAX_COPY_SIDE,
                    image::imageops::FilterType::Triangle,
                );
            }
            // The decoded image goes as its pixels are taken.
            let rgba = img.into_rgba8();
            let size = [rgba.width() as usize, rgba.height() as usize];
            Ok(egui::ColorImage::from_rgba_unmultiplied(
                size,
                rgba.as_raw(),
            ))
        };
        let result = read()
            .map_err(|e| format!("This photo could not be read to copy ({e}): open it instead."));
        let _ = tx.send(result);
    });
    rx
}

impl PhotosView {
    /// What this page said happened since the last call, for the app's one
    /// notice (E5).
    pub fn take_notice(&mut self) -> Option<crate::notice::Notice> {
        self.notice.take()
    }
    fn refresh(&mut self, state_dir: &Path) {
        if self.read_at.is_none_or(|t| t.elapsed() >= REFRESH) {
            self.kept = store::read(state_dir);
            self.read_at = Some(Instant::now());
        }
    }

    fn ask(&mut self, state_dir: &Path, device: &str, ask: Ask, now_unix: u64) -> bool {
        match store::request(state_dir, device, ask, now_unix) {
            Ok(_) => true,
            Err(e) => {
                self.notice = crate::notice::Notice::error(format!("Could not ask the phone: {e}"));
                false
            }
        }
    }

    /// The photos part of the Phone page, for `phone`.
    pub fn show(&mut self, ui: &mut egui::Ui, state_dir: &Path, phone: &str, now_unix: u64) {
        self.refresh(state_dir);
        ui.add_space(12.0);
        ui.strong("Recent photos");
        let kept = self.kept.get(phone).cloned();
        if let Some((asking, asked)) = self.waiting.clone()
            && asking == phone
        {
            if kept.as_ref().is_some_and(|k| k.at_unix >= asked) {
                self.waiting = None;
            } else if now_unix.saturating_sub(asked) > ANSWER_SECS {
                self.waiting = None;
                self.notice = crate::notice::Notice::error(
                    "The phone did not answer. Turn on \"Recent photos on the computer\" in \
                     Pocketdesk's Settings on the phone, and \"Send and browse files\" for it in Devices.",
                );
            }
        }
        self.finish(ui.ctx(), state_dir, now_unix);
        ui.horizontal_wrapped(|ui| {
            let label = if kept.is_some() {
                "Refresh photos"
            } else {
                "Show the phone's recent photos"
            };
            if ui.button(label).clicked() && self.ask(state_dir, phone, Ask::List, now_unix) {
                self.waiting = Some((phone.to_string(), now_unix));
                self.notice = crate::notice::Notice::done("Asked the phone for its recent photos…");
            }
            if self.waiting.as_ref().is_some_and(|(p, _)| p == phone) {
                ui.spinner();
            }
        });
        let Some(kept) = kept else {
            return;
        };
        if let Some(error) = &kept.error {
            ui.label(RichText::new(error).color(crate::theme::bad(ui)));
        }
        if kept.photos.is_empty() && kept.error.is_none() {
            ui.label(
                RichText::new("No camera photos on the phone.").color(crate::theme::muted(ui)),
            );
            return;
        }
        let mut action = None;
        ui.horizontal_wrapped(|ui| {
            for photo in &kept.photos {
                ui.vertical(|ui| {
                    ui.set_width(TILE);
                    let key = (phone.to_string(), photo.id, kept.at_unix);
                    let texture = self
                        .textures
                        .entry(key)
                        .or_insert_with(|| {
                            thumbnail(ui.ctx(), state_dir, phone, photo.id, photo.thumb)
                        })
                        .clone();
                    match texture {
                        Some(texture) => {
                            ui.add(
                                egui::Image::new(&texture)
                                    .fit_to_exact_size(egui::vec2(TILE, TILE))
                                    // Its name for a screen reader (E5).
                                    .alt_text(&photo.name),
                            )
                            .on_hover_text(&photo.name);
                        }
                        None => {
                            ui.add_sized(
                                [TILE, TILE],
                                egui::Label::new(
                                    RichText::new("No preview").color(crate::theme::muted(ui)),
                                ),
                            );
                        }
                    }
                    ui.label(
                        RichText::new(taken_text(photo.taken, now_unix))
                            .small()
                            .color(crate::theme::muted(ui)),
                    )
                    .on_hover_text(&photo.name);
                    ui.horizontal(|ui| {
                        // Each says which photo to a screen reader (E5):
                        // a page of "Save" said nothing of which.
                        let named = |response: egui::Response, what: &str| {
                            let label = format!("{what} {}", photo.name);
                            response.widget_info(|| {
                                egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &label)
                            });
                            response
                        };
                        if named(ui.small_button("Save"), "Save").clicked() {
                            action = Some((photo.id, photo.name.clone(), photo.size, Then::Save));
                        }
                        if openable(&photo.name) && named(ui.small_button("Open"), "Open").clicked()
                        {
                            action = Some((photo.id, photo.name.clone(), photo.size, Then::Open));
                        }
                        if copyable(&photo.name) && named(ui.small_button("Copy"), "Copy").clicked()
                        {
                            action = Some((photo.id, photo.name.clone(), photo.size, Then::Copy));
                        }
                    });
                });
            }
        });
        if let Some((id, name, size, then)) = action {
            self.act(state_dir, phone, id, name, size, then, now_unix);
        }
        // Thumbnails of older answers are not drawn again.
        let at = kept.at_unix;
        self.textures
            .retain(|(p, _, when), _| p != phone || *when == at);
    }

    /// Ask for a photo, unless it is already here for Open or Copy.
    #[allow(clippy::too_many_arguments)]
    fn act(
        &mut self,
        state_dir: &Path,
        phone: &str,
        id: u64,
        name: String,
        size: u64,
        then: Then,
        now_unix: u64,
    ) {
        let rows = received::read(state_dir);
        // A photo saved earlier in this run (since the answer came) is used as it is.
        let since = self.kept.get(phone).map_or(now_unix, |k| k.at_unix);
        if then != Then::Save
            && let Some(path) = arrived(&rows, phone, &name, size, since)
        {
            self.then(path, &name, then);
            return;
        }
        if self.ask(state_dir, phone, Ask::Fetch(id), now_unix) {
            self.notice = crate::notice::Notice::done(format!("Asked the phone for {name}…"));
            self.pending.push(Pending {
                device: phone.to_string(),
                name,
                size,
                then,
                asked_unix: now_unix,
            });
        }
    }

    /// Do what a photo was fetched for, now it is at `path`.
    fn then(&mut self, path: PathBuf, name: &str, then: Then) {
        match then {
            Then::Save => {
                self.notice =
                    crate::notice::Notice::done(format!("Saved {name} to {}.", path.display()))
            }
            Then::Open => {
                self.notice = match open_file(&path) {
                    Ok(()) => crate::notice::Notice::done(format!("Opened {name}.")),
                    Err(e) => crate::notice::Notice::error(format!("Could not open {name}: {e}")),
                };
            }
            Then::Copy => {
                self.notice = crate::notice::Notice::done(format!("Copying {name}…"));
                self.copying = Some(decode(path));
            }
        }
    }

    /// Photos that arrived, or waited too long; a decoded copy put on the clipboard.
    fn finish(&mut self, ctx: &egui::Context, state_dir: &Path, now_unix: u64) {
        if !self.pending.is_empty() {
            let rows = received::read(state_dir);
            let mut done = Vec::new();
            for (i, p) in self.pending.iter().enumerate() {
                // Past its time it is dropped even if it came since: Open or
                // Copy long after the click (the page left and come back to)
                // would surprise the owner (C7 review).
                if now_unix.saturating_sub(p.asked_unix) > FETCH_SECS {
                    done.push((i, None));
                } else if let Some(path) = arrived(&rows, &p.device, &p.name, p.size, p.asked_unix)
                {
                    done.push((i, Some(path)));
                }
            }
            for (i, path) in done.into_iter().rev() {
                let p = self.pending.remove(i);
                match path {
                    Some(path) => self.then(path, &p.name, p.then),
                    None => {
                        self.notice = crate::notice::Notice::error(format!(
                            "{} did not arrive. Is the phone connected, with Recent photos on?",
                            p.name
                        ))
                    }
                }
            }
            ctx.request_repaint_after(REFRESH);
        }
        if let Some(rx) = &self.copying {
            match rx.try_recv() {
                Ok(Ok(image)) => {
                    ctx.copy_image(image);
                    self.notice = crate::notice::Notice::done("Copied.");
                    self.copying = None;
                }
                Ok(Err(e)) => {
                    self.notice = crate::notice::Notice::error(e);
                    self.copying = None;
                }
                Err(mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(Duration::from_millis(100))
                }
                Err(mpsc::TryRecvError::Disconnected) => self.copying = None,
            }
        }
    }
}

/// A photo's thumbnail as a texture, if it was kept and reads as a picture.
fn thumbnail(
    ctx: &egui::Context,
    state_dir: &Path,
    phone: &str,
    id: u64,
    kept: bool,
) -> Option<egui::TextureHandle> {
    if !kept {
        return None;
    }
    let path = store::thumb_path(state_dir, phone, id)?;
    let bytes = std::fs::read(path).ok()?;
    // Held to a thumbnail's size before decoding: a few kilobytes can
    // declare a picture the GPU refuses, or gigabytes (C7 review).
    let mut reader =
        image::ImageReader::with_format(std::io::Cursor::new(bytes), image::ImageFormat::Jpeg);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(store::MAX_THUMB_SIDE);
    limits.max_image_height = Some(store::MAX_THUMB_SIDE);
    limits.max_alloc = Some(8 << 20);
    reader.limits(limits);
    let img = reader.decode().ok()?;
    let rgba = img.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    let color = egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
    Some(ctx.load_texture(
        format!("photo-{phone}-{id}"),
        color,
        egui::TextureOptions::LINEAR,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(name: &str, from: &str, at: u64) -> ReceivedFile {
        ReceivedFile {
            name: name.into(),
            path: format!("C:/Downloads/{name}"),
            size: 1,
            received_unix: at,
            from: from.into(),
        }
    }

    #[test]
    fn a_fetched_photo_is_found_by_its_phone_name_and_time() {
        let rows = [
            row("IMG_1.jpg", "phone-a", 90),
            row("IMG_1 (1).jpg", "phone-b", 120),
            row("IMG_1 (2).jpg", "phone-a", 120),
        ];
        assert_eq!(
            arrived(&rows, "phone-a", "IMG_1.jpg", 0, 100),
            Some(PathBuf::from("C:/Downloads/IMG_1 (2).jpg")),
            "the newest, saved beside an older of that name"
        );
        assert_eq!(
            arrived(&rows, "phone-a", "IMG_1.jpg", 0, 130),
            None,
            "none since"
        );
        assert_eq!(
            arrived(&rows, "phone-c", "IMG_1.jpg", 0, 0),
            None,
            "another phone's"
        );
        assert_eq!(
            arrived(&[row("IMG_10.jpg", "p", 5)], "p", "IMG_1.jpg", 0, 0),
            None
        );
        assert_eq!(
            arrived(&[row("IMG_1 (x).jpg", "p", 5)], "p", "IMG_1.jpg", 0, 0),
            None
        );
    }

    #[test]
    fn two_photos_of_one_name_are_told_apart_by_size() {
        assert_eq!(
            arrived(&[row("IMG_1.jpg", "p", 5)], "p", "IMG_1.jpg", 2, 0),
            None
        );
        assert!(arrived(&[row("IMG_1.jpg", "p", 5)], "p", "IMG_1.jpg", 1, 0).is_some());
    }

    #[test]
    fn only_a_picture_is_opened() {
        for name in ["IMG_1.jpg", "a.JPEG", "b.png", "c.heic", "d.webp"] {
            assert!(openable(name), "{name}");
        }
        for name in ["setup.exe", "photo.jpg.exe", "x.lnk", "README", "a.svg"] {
            assert!(!openable(name), "{name}");
        }
    }

    /// The grid from a phone's answer, driven headless (C4's harness): each
    /// photo's thumbnail and its Save, Open and Copy.
    #[test]
    fn the_grid_shows_each_photo_with_its_actions() {
        use egui_kittest::kittest::Queryable;
        use pocketdesk_core::protocol::phone_photos::{Photo, Photos};
        let dir = std::env::temp_dir().join(format!("pd-photos-view-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // A real JPEG, so the thumbnail decodes as the page's would.
        let mut jpeg = Vec::new();
        image::RgbImage::from_pixel(8, 6, image::Rgb([200, 80, 40]))
            .write_to(
                &mut std::io::Cursor::new(&mut jpeg),
                image::ImageFormat::Jpeg,
            )
            .unwrap();
        use base64::Engine;
        let thumb = base64::engine::general_purpose::STANDARD.encode(&jpeg);
        let photo = |id: u64, name: &str| Photo {
            id,
            name: name.into(),
            taken: 0,
            size: 10,
            thumb: thumb.clone(),
        };
        let answer = Photos {
            photos: vec![photo(1, "IMG_1.jpg"), photo(2, "clip.mp4.exe")],
            error: None,
        };
        store::keep(&dir, "phone-a", answer, 100, &|_| true).unwrap();

        let state = dir.clone();
        let mut harness = egui_kittest::Harness::new_ui_state(
            move |ui, view: &mut PhotosView| view.show(ui, &state, "phone-a", 100),
            PhotosView::default(),
        );
        harness.run_steps(3);
        assert!(harness.query_by_label("Refresh photos").is_some());
        // Each button names its photo (E5).
        assert!(harness.query_by_label("Save IMG_1.jpg").is_some());
        assert!(harness.query_by_label("Save clip.mp4.exe").is_some());
        // Only the picture may be opened or copied.
        assert!(harness.query_by_label("Copy IMG_1.jpg").is_some());
        assert!(harness.query_by_label("Open IMG_1.jpg").is_some());
        assert!(harness.query_by_label("Open clip.mp4.exe").is_none());
        assert!(harness.query_by_label("Copy clip.mp4.exe").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_a_format_the_app_decodes_is_copied() {
        for name in ["IMG_1.jpg", "a.JPEG", "b.png", "c.webp"] {
            assert!(copyable(name), "{name}");
        }
        for name in ["c.heic", "d.gif", "e.bmp", "x.exe"] {
            assert!(!copyable(name), "{name}");
        }
    }

    /// A huge photo is copied at most MAX_COPY_SIDE a side. (Turning by
    /// the camera's orientation is the image crate's, given the tag.)
    #[test]
    fn a_copied_photo_is_bounded() {
        let dir = std::env::temp_dir().join(format!("pd-photos-copy-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let wide = dir.join("wide.png");
        image::RgbImage::from_pixel(MAX_COPY_SIDE * 2, 10, image::Rgb([1, 2, 3]))
            .save(&wide)
            .unwrap();
        let copied = decode(wide).recv().unwrap().unwrap();
        assert_eq!(copied.size[0], MAX_COPY_SIDE as usize);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn when_a_photo_was_taken_reads_as_how_long_ago() {
        assert_eq!(taken_text(0, 1_000), "");
        assert!(!taken_text(500_000, 1_000).is_empty());
    }
}
