//! The other computer's pointer in the window that shows its screen (local
//! cursor): what to draw, each pass, from what that computer last said of
//! its pointer ([`pocketdesk_core::protocol::cursor`]).
//!
//! With the pointer free (the usual way: this computer's pointer is the
//! other's, one to one), the shape becomes this window's own system
//! pointer while the pointer is over the picture: the compositor draws it
//! where the mouse is, so it moves with no wait for any picture. With the
//! pointer locked to the window (relative moves, PC7) this computer's
//! pointer is hidden and says nothing of where the other's is, so the
//! shape is drawn over the picture where that computer says its pointer is.
//!
//! Nothing is hidden or replaced unless the other computer said its
//! picture has no pointer in it ([`CursorState::separate`]): an older one,
//! or one that cannot take the pointer out, leaves the window as it was.

use std::sync::Arc;

use eframe::egui;
use pocketdesk_core::protocol::cursor::{CursorShape, CursorState};

/// What the window does with the pointer this pass.
#[derive(Debug, Clone, PartialEq)]
pub enum Draw {
    /// Nothing: the window's pointer is as it was.
    Leave,
    /// The pointer over the picture is this shape.
    System(Arc<CursorShape>),
    /// The pointer over the picture is hidden (the other computer's is).
    Hide,
    /// This shape is painted over the picture at `at` (the hotspot's place
    /// in the window), the system pointer being locked away.
    Paint {
        shape: Arc<CursorShape>,
        at: egui::Pos2,
    },
}

/// The window's pointer for the other computer's `state`, over `picture`
/// (`None` while none is shown), with this computer's pointer at `pointer`
/// (`None` outside the window) and `locked` to the window or not.
pub fn plan(
    state: &CursorState,
    picture: Option<egui::Rect>,
    pointer: Option<egui::Pos2>,
    locked: bool,
) -> Draw {
    let Some(picture) = picture else {
        return Draw::Leave;
    };
    if !state.separate {
        return Draw::Leave;
    }
    if locked {
        // This computer's own pointer is locked away, so what is painted is
        // all there is of a pointer: a shape that has not come stands in as
        // an arrow, rather than leave none. Only a pointer the other
        // computer hides, or one it has not said a place for, is not drawn.
        return match (state.place, state.visible) {
            (Some((x, y)), true) => Draw::Paint {
                shape: state.shape.clone().unwrap_or_else(fallback_shape),
                at: place_in(picture, x, y),
            },
            _ => Draw::Leave,
        };
    }
    if !pointer.is_some_and(|p| picture.contains(p)) {
        return Draw::Leave;
    }
    match (&state.shape, state.visible) {
        (_, false) => Draw::Hide,
        (Some(shape), true) => Draw::System(shape.clone()),
        // Shown, but its shape has not come: the window's own pointer
        // stands in.
        (None, true) => Draw::Leave,
    }
}

/// A plain arrow, hotspot at its tip: painted for a pointer that is locked
/// away here when the other computer has not sent its shape (yet, or at
/// all), so that no pointer is ever missing from the window.
pub fn fallback_shape() -> Arc<CursorShape> {
    static ARROW: std::sync::OnceLock<Arc<CursorShape>> = std::sync::OnceLock::new();
    ARROW
        .get_or_init(|| {
            // X is the outline, O the fill, a space is clear.
            const ROWS: [&str; 17] = [
                "X          ",
                "XX         ",
                "XOX        ",
                "XOOX       ",
                "XOOOX      ",
                "XOOOOX     ",
                "XOOOOOX    ",
                "XOOOOOOX   ",
                "XOOOOOOOX  ",
                "XOOOOOOOOX ",
                "XOOOOOXXXXX",
                "XOOXOOX    ",
                "XOX XOOX   ",
                "XX  XOOX   ",
                "X    XOOX  ",
                "     XOOX  ",
                "      XX   ",
            ];
            let mut rgba = Vec::with_capacity(11 * ROWS.len() * 4);
            for row in ROWS {
                let row = row.as_bytes();
                for x in 0..11 {
                    rgba.extend_from_slice(match row.get(x) {
                        Some(b'X') => &[0, 0, 0, 255],
                        Some(b'O') => &[255, 255, 255, 255],
                        _ => &[0, 0, 0, 0],
                    });
                }
            }
            Arc::new(
                CursorShape::new(11, ROWS.len() as u32, 0, 0, rgba)
                    .expect("the arrow's pixels add up"),
            )
        })
        .clone()
}

/// Where the hotspot at `x`, `y` (`0..=65535` over the picture) is in the
/// window.
pub fn place_in(picture: egui::Rect, x: u16, y: u16) -> egui::Pos2 {
    egui::pos2(
        picture.left() + f32::from(x) / 65535.0 * picture.width(),
        picture.top() + f32::from(y) / 65535.0 * picture.height(),
    )
}

/// The shape as the window system's pointer: the image and its hotspot.
pub fn system_image(shape: &CursorShape) -> egui::CustomCursorImage {
    egui::CustomCursorImage {
        rgba: shape.rgba.clone(),
        size: [shape.width, shape.height],
        hotspot: [shape.hot_x, shape.hot_y],
    }
}

/// The rectangle a painted shape fills, in points: its image, its hotspot
/// at `at`.
pub fn paint_rect(shape: &CursorShape, at: egui::Pos2, pixels_per_point: f32) -> egui::Rect {
    let scale = 1.0 / pixels_per_point.max(0.1);
    let top_left = at - egui::vec2(f32::from(shape.hot_x), f32::from(shape.hot_y)) * scale;
    egui::Rect::from_min_size(
        top_left,
        egui::vec2(f32::from(shape.width), f32::from(shape.height)) * scale,
    )
}

/// Whether the window asks the other computer for its pointer's places now:
/// while its pointer is locked, and only of a computer that sends the
/// pointer apart.
pub fn wants_places(state: &CursorState, locked: bool) -> bool {
    locked && state.separate
}

#[cfg(test)]
mod tests {
    use super::*;
    use pocketdesk_core::protocol::cursor::CursorMessage;

    fn shape(seed: u8) -> CursorShape {
        CursorShape::new(4, 4, 1, 2, vec![seed; 64]).unwrap()
    }

    fn rect() -> egui::Rect {
        egui::Rect::from_min_size(egui::pos2(100.0, 50.0), egui::vec2(800.0, 400.0))
    }

    fn state(
        separate: bool,
        visible: bool,
        with_shape: bool,
        place: Option<(u16, u16)>,
    ) -> CursorState {
        let mut s = CursorState::default();
        s.apply(CursorMessage::Mode { separate });
        if with_shape {
            s.apply(CursorMessage::Shape(shape(7)));
        }
        s.apply(CursorMessage::Visible(visible));
        if let Some((x, y)) = place {
            s.apply(CursorMessage::Move { x, y });
        }
        s
    }

    #[test]
    fn a_computer_that_keeps_its_pointer_in_the_picture_changes_nothing() {
        let s = state(false, true, true, None);
        assert_eq!(
            plan(&s, Some(rect()), Some(egui::pos2(200.0, 100.0)), false),
            Draw::Leave
        );
        assert_eq!(plan(&s, Some(rect()), None, true), Draw::Leave);
        let fresh = CursorState::default();
        assert_eq!(
            plan(&fresh, Some(rect()), Some(egui::pos2(200.0, 100.0)), false),
            Draw::Leave
        );
    }

    #[test]
    fn a_free_pointer_over_the_picture_wears_the_other_computers_shape() {
        let s = state(true, true, true, None);
        let over = Some(egui::pos2(200.0, 100.0));
        assert!(matches!(
            plan(&s, Some(rect()), over, false),
            Draw::System(_)
        ));
        assert_eq!(
            plan(&s, Some(rect()), Some(egui::pos2(10.0, 10.0)), false),
            Draw::Leave,
            "off the picture it is this computer's own"
        );
        assert_eq!(
            plan(&s, Some(rect()), None, false),
            Draw::Leave,
            "outside the window"
        );
        assert_eq!(plan(&s, None, over, false), Draw::Leave, "no picture");
    }

    #[test]
    fn a_pointer_the_other_computer_hides_is_hidden_and_one_without_a_shape_yet_is_left() {
        let over = Some(egui::pos2(200.0, 100.0));
        assert_eq!(
            plan(&state(true, false, true, None), Some(rect()), over, false),
            Draw::Hide
        );
        assert_eq!(
            plan(&state(true, true, false, None), Some(rect()), over, false),
            Draw::Leave
        );
    }

    #[test]
    fn a_locked_pointer_is_painted_where_the_other_computer_says_it_is() {
        let s = state(true, true, true, Some((0, 65535)));
        match plan(&s, Some(rect()), None, true) {
            Draw::Paint { at, .. } => {
                assert_eq!(at, egui::pos2(100.0, 450.0), "the picture's bottom left");
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(
            plan(&state(true, true, true, None), Some(rect()), None, true),
            Draw::Leave,
            "no place told yet"
        );
        // The shape has not come: an arrow stands in, never no pointer.
        match plan(
            &state(true, true, false, Some((65535, 0))),
            Some(rect()),
            None,
            true,
        ) {
            Draw::Paint { shape, at } => {
                assert_eq!(shape.id, fallback_shape().id);
                assert_eq!(at, egui::pos2(900.0, 50.0), "the picture's top right");
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(
            plan(
                &state(true, false, true, Some((5, 5))),
                Some(rect()),
                None,
                true
            ),
            Draw::Leave,
            "hidden there"
        );
    }

    #[test]
    fn the_stand_in_arrow_is_a_real_image_with_its_tip_as_the_hotspot() {
        let arrow = fallback_shape();
        assert_eq!((arrow.width, arrow.height), (11, 17));
        assert_eq!((arrow.hot_x, arrow.hot_y), (0, 0));
        assert_eq!(arrow.rgba.len(), 11 * 17 * 4);
        assert_eq!(&arrow.rgba[..4], &[0, 0, 0, 255], "the tip is outlined");
        assert!(Arc::ptr_eq(&arrow, &fallback_shape()), "made once");
    }

    #[test]
    fn a_painted_shape_puts_its_hotspot_at_the_place_and_scales_with_the_screen() {
        let s = shape(1);
        let at = egui::pos2(300.0, 200.0);
        let r = paint_rect(&s, at, 1.0);
        assert_eq!(r.min, egui::pos2(299.0, 198.0), "the hotspot is (1, 2)");
        assert_eq!(r.size(), egui::vec2(4.0, 4.0));
        let r = paint_rect(&s, at, 2.0);
        assert_eq!(r.min, egui::pos2(299.5, 199.0));
        assert_eq!(
            r.size(),
            egui::vec2(2.0, 2.0),
            "half the points on a 2x screen"
        );
    }

    #[test]
    fn the_system_pointer_takes_the_image_and_its_hotspot() {
        let s = shape(9);
        let image = system_image(&s);
        assert_eq!(image.size, [4, 4]);
        assert_eq!(image.hotspot, [1, 2]);
        assert!(
            Arc::ptr_eq(&image.rgba, &s.rgba),
            "no copy a pass: the same image is the same pointer"
        );
    }

    #[test]
    fn places_are_asked_for_only_while_locked_and_only_of_a_computer_that_sends_them() {
        assert!(wants_places(&state(true, true, true, None), true));
        assert!(!wants_places(&state(true, true, true, None), false));
        assert!(!wants_places(&state(false, true, true, None), true));
        assert!(!wants_places(&CursorState::default(), true));
    }
}
