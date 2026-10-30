//! NV12 pictures (PC5): a byte of luma a pixel, then a plane of half the
//! width and half the height (rounded up) of interleaved Cb and Cr, 12
//! bits a pixel against RGBA's 32. Their layout, and their colours turned
//! to RGB as the stream says (BT.601 or BT.709, limited or full range):
//! by a small shader in an egui paint callback where the app draws with
//! glow, else here, on the processor (a screenshot, another renderer, a
//! shader that failed).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use eframe::glow::HasContext as _;
use eframe::{egui, egui_glow, glow};

use crate::video::{Colour, Picture, Planes};

/// Where an NV12 picture's planes lie in its bytes, as ffmpeg's rawvideo
/// writes them: rows without padding, the luma plane `width` bytes a row,
/// the chroma plane `2 × chroma_width` bytes a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    pub width: usize,
    pub height: usize,
    pub chroma_width: usize,
    pub chroma_height: usize,
}

impl Layout {
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            chroma_width: width.div_ceil(2),
            chroma_height: height.div_ceil(2),
        }
    }

    /// The luma plane's bytes.
    pub fn y_len(&self) -> usize {
        self.width * self.height
    }

    /// A chroma row's bytes: Cb and Cr for each pair of pixels.
    pub fn uv_stride(&self) -> usize {
        self.chroma_width * 2
    }

    /// The chroma plane's bytes.
    pub fn uv_len(&self) -> usize {
        self.uv_stride() * self.chroma_height
    }

    /// The picture's bytes.
    pub fn len(&self) -> usize {
        self.y_len() + self.uv_len()
    }

    /// The luma and chroma planes of `data`, when it is this picture's
    /// size.
    pub fn split<'a>(&self, data: &'a [u8]) -> Option<(&'a [u8], &'a [u8])> {
        (data.len() == self.len()).then(|| data.split_at(self.y_len()))
    }
}

/// The conversion from samples (0 to 1) to RGB for `colour`: a matrix,
/// column by column as GLSL's `mat3` takes it, applied to the samples less
/// the offsets.
pub fn coefficients(colour: Colour) -> ([f32; 9], [f32; 3]) {
    let (kr, kb) = if colour.bt709 {
        (0.2126f32, 0.0722f32)
    } else {
        (0.299f32, 0.114f32)
    };
    let kg = 1.0 - kr - kb;
    let (ys, cs, y0) = if colour.full {
        (1.0f32, 1.0f32, 0.0f32)
    } else {
        (255.0 / 219.0, 255.0 / 224.0, 16.0 / 255.0)
    };
    let c0 = 128.0 / 255.0;
    let rv = cs * 2.0 * (1.0 - kr);
    let gu = -cs * 2.0 * kb * (1.0 - kb) / kg;
    let gv = -cs * 2.0 * kr * (1.0 - kr) / kg;
    let bu = cs * 2.0 * (1.0 - kb);
    // Columns: what Y, Cb and Cr each add to R, G and B.
    ([ys, ys, ys, 0.0, gu, bu, rv, gv, 0.0], [y0, c0, c0])
}

/// An NV12 picture as RGBA, opaque: `data` in `layout`, its colours as
/// `colour` says. Empty when `data` is not that size.
pub fn to_rgba(layout: &Layout, data: &[u8], colour: Colour) -> Vec<u8> {
    let Some((y_plane, uv_plane)) = layout.split(data) else {
        return Vec::new();
    };
    let (m, off) = coefficients(colour);
    let mut rgba = vec![0u8; layout.width * layout.height * 4];
    let to_byte = |v: f32| (v * 255.0).round().clamp(0.0, 255.0) as u8;
    for row in 0..layout.height {
        let uv_row = &uv_plane[(row / 2) * layout.uv_stride()..][..layout.uv_stride()];
        for col in 0..layout.width {
            let y = f32::from(y_plane[row * layout.width + col]) / 255.0 - off[0];
            let u = f32::from(uv_row[(col / 2) * 2]) / 255.0 - off[1];
            let v = f32::from(uv_row[(col / 2) * 2 + 1]) / 255.0 - off[2];
            let at = (row * layout.width + col) * 4;
            rgba[at] = to_byte(m[0] * y + m[3] * u + m[6] * v);
            rgba[at + 1] = to_byte(m[1] * y + m[4] * u + m[7] * v);
            rgba[at + 2] = to_byte(m[2] * y + m[5] * u + m[8] * v);
            rgba[at + 3] = 255;
        }
    }
    rgba
}

/// Whether the app draws with glow (OpenGL), set once as it starts; and
/// whether the NV12 shader failed here, for good.
static GLOW: AtomicBool = AtomicBool::new(false);
static FAILED: AtomicBool = AtomicBool::new(false);

/// The app draws with glow (`true`) or another renderer.
pub fn set_glow(on: bool) {
    GLOW.store(on, Ordering::Relaxed);
}

/// The planes a window asks its decoder for: NV12 while glow draws it and
/// its shader has not failed, else RGBA (PC5).
pub fn choose(glow: bool, failed: bool) -> Planes {
    if glow && !failed {
        Planes::Nv12
    } else {
        Planes::Rgba
    }
}

/// The planes to ask for now.
pub fn planes() -> Planes {
    choose(GLOW.load(Ordering::Relaxed), FAILED.load(Ordering::Relaxed))
}

/// The vertex shader: a rectangle over the callback's viewport from four
/// vertices and no buffer, the picture's top row at the top.
const VERTEX: &str = "
out vec2 v_at;
void main() {
    vec2 p = vec2(float(gl_VertexID & 1), float(gl_VertexID >> 1));
    v_at = p;
    gl_Position = vec4(p.x * 2.0 - 1.0, 1.0 - p.y * 2.0, 0.0, 1.0);
}
";

/// The fragment shader: luma and chroma to RGB by [`coefficients`]'
/// matrix, opaque, tinted (egui's colours are premultiplied).
const FRAGMENT: &str = "
#ifdef GL_ES
precision highp float;
#endif
uniform sampler2D u_y;
uniform sampler2D u_uv;
uniform mat3 u_m;
uniform vec3 u_off;
uniform vec4 u_tint;
in vec2 v_at;
out vec4 out_colour;
void main() {
    float y = texture(u_y, v_at).r;
    vec2 c = texture(u_uv, v_at).rg;
    vec3 rgb = clamp(u_m * (vec3(y, c) - u_off), 0.0, 1.0);
    out_colour = vec4(rgb, 1.0) * u_tint;
}
";

/// An NV12 picture on the GPU, drawn in an egui paint callback (PC5): its
/// planes are uploaded as two textures (one byte a texel, then two) as a
/// picture comes, and turned to RGB by a small shader as it is drawn. Its
/// GL objects live as long as the window's view (one GL context, shared by
/// the app's windows).
#[derive(Default)]
pub struct Gpu {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    pending: Option<Arc<Picture>>,
    gl: Option<Drawn>,
}

impl Gpu {
    /// The next picture to draw.
    pub fn put(&self, picture: Arc<Picture>) {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).pending = Some(picture);
    }

    /// Draw the last picture put over `rect`, tinted.
    pub fn paint(self: &Arc<Self>, rect: egui::Rect, tint: egui::Color32) -> egui::PaintCallback {
        let gpu = self.clone();
        let tint = tint.to_normalized_gamma_f32();
        egui::PaintCallback {
            rect,
            callback: Arc::new(egui_glow::CallbackFn::new(move |_info, painter| {
                gpu.draw(painter.gl(), tint);
            })),
        }
    }

    fn draw(&self, gl: &glow::Context, tint: [f32; 4]) {
        if FAILED.load(Ordering::Relaxed) {
            return;
        }
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if inner.gl.is_none() {
            // SAFETY: called from egui_glow's paint callback, with its GL
            // context current on this thread.
            match unsafe { Drawn::new(gl) } {
                Ok(drawn) => inner.gl = Some(drawn),
                Err(e) => {
                    eprintln!("Pocketdesk: the NV12 shader failed ({e}); drawing RGBA");
                    FAILED.store(true, Ordering::Relaxed);
                    return;
                }
            }
        }
        let pending = inner.pending.take();
        let Some(drawn) = inner.gl.as_mut() else {
            return;
        };
        // SAFETY: as above; the textures and program are this context's.
        unsafe {
            if let Some(picture) = pending {
                drawn.upload(gl, &picture);
            }
            drawn.draw(gl, tint);
        }
    }
}

/// The shader, its vertex array and the two planes' textures.
struct Drawn {
    program: glow::Program,
    vao: glow::VertexArray,
    y: glow::Texture,
    uv: glow::Texture,
    size: Option<(usize, usize)>,
    colour: Colour,
    u_y: Option<glow::UniformLocation>,
    u_uv: Option<glow::UniformLocation>,
    u_m: Option<glow::UniformLocation>,
    u_off: Option<glow::UniformLocation>,
    u_tint: Option<glow::UniformLocation>,
}

impl Drawn {
    /// Compile and link the shader and make the textures; why not, when
    /// the context cannot (GLSL before 1.40 or ES 3.0).
    unsafe fn new(gl: &glow::Context) -> Result<Self, String> {
        let version = egui_glow::ShaderVersion::get(gl);
        if !version.is_new_shader_interface() {
            return Err(format!("{version:?} has no in/out or RG textures"));
        }
        let header = version.version_declaration();
        unsafe {
            let program = gl.create_program()?;
            let mut shaders = Vec::new();
            for (kind, source) in [
                (glow::VERTEX_SHADER, VERTEX),
                (glow::FRAGMENT_SHADER, FRAGMENT),
            ] {
                let shader = gl.create_shader(kind)?;
                gl.shader_source(shader, &format!("{header}{source}"));
                gl.compile_shader(shader);
                if !gl.get_shader_compile_status(shader) {
                    let log = gl.get_shader_info_log(shader);
                    gl.delete_shader(shader);
                    gl.delete_program(program);
                    return Err(log);
                }
                gl.attach_shader(program, shader);
                shaders.push(shader);
            }
            gl.link_program(program);
            for shader in shaders {
                gl.detach_shader(program, shader);
                gl.delete_shader(shader);
            }
            if !gl.get_program_link_status(program) {
                let log = gl.get_program_info_log(program);
                gl.delete_program(program);
                return Err(log);
            }
            let vao = gl.create_vertex_array()?;
            let texture = || -> Result<glow::Texture, String> {
                let t = gl.create_texture()?;
                gl.bind_texture(glow::TEXTURE_2D, Some(t));
                for (name, value) in [
                    (glow::TEXTURE_MIN_FILTER, glow::LINEAR),
                    (glow::TEXTURE_MAG_FILTER, glow::LINEAR),
                    (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
                    (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
                ] {
                    gl.tex_parameter_i32(glow::TEXTURE_2D, name, value as i32);
                }
                Ok(t)
            };
            let (y, uv) = (texture()?, texture()?);
            gl.bind_texture(glow::TEXTURE_2D, None);
            Ok(Self {
                u_y: gl.get_uniform_location(program, "u_y"),
                u_uv: gl.get_uniform_location(program, "u_uv"),
                u_m: gl.get_uniform_location(program, "u_m"),
                u_off: gl.get_uniform_location(program, "u_off"),
                u_tint: gl.get_uniform_location(program, "u_tint"),
                program,
                vao,
                y,
                uv,
                size: None,
                colour: Colour::default(),
            })
        }
    }

    /// Upload `picture`'s planes, making the textures again at a new size.
    unsafe fn upload(&mut self, gl: &glow::Context, picture: &Picture) {
        let layout = Layout::new(picture.width, picture.height);
        let Some((y_plane, uv_plane)) = layout.split(&picture.data) else {
            return;
        };
        let new_size = self.size != Some((picture.width, picture.height));
        self.size = Some((picture.width, picture.height));
        self.colour = picture.colour;
        unsafe {
            let alignment = gl.get_parameter_i32(glow::UNPACK_ALIGNMENT);
            // Rows of any width, unpadded.
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
            gl.active_texture(glow::TEXTURE1);
            for (texture, internal, format, w, h, plane) in [
                (
                    self.y,
                    glow::R8,
                    glow::RED,
                    layout.width,
                    layout.height,
                    y_plane,
                ),
                (
                    self.uv,
                    glow::RG8,
                    glow::RG,
                    layout.chroma_width,
                    layout.chroma_height,
                    uv_plane,
                ),
            ] {
                gl.bind_texture(glow::TEXTURE_2D, Some(texture));
                let (w, h) = (w as i32, h as i32);
                let pixels = glow::PixelUnpackData::Slice(Some(plane));
                if new_size {
                    gl.tex_image_2d(
                        glow::TEXTURE_2D,
                        0,
                        internal as i32,
                        w,
                        h,
                        0,
                        format,
                        glow::UNSIGNED_BYTE,
                        pixels,
                    );
                } else {
                    gl.tex_sub_image_2d(
                        glow::TEXTURE_2D,
                        0,
                        0,
                        0,
                        w,
                        h,
                        format,
                        glow::UNSIGNED_BYTE,
                        pixels,
                    );
                }
            }
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.active_texture(glow::TEXTURE0);
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, alignment);
        }
    }

    /// Draw the last picture uploaded over the callback's viewport; egui
    /// sets its own state again after.
    unsafe fn draw(&self, gl: &glow::Context, tint: [f32; 4]) {
        if self.size.is_none() {
            return;
        }
        let (m, off) = coefficients(self.colour);
        unsafe {
            gl.use_program(Some(self.program));
            gl.active_texture(glow::TEXTURE1);
            gl.bind_texture(glow::TEXTURE_2D, Some(self.y));
            gl.active_texture(glow::TEXTURE2);
            gl.bind_texture(glow::TEXTURE_2D, Some(self.uv));
            gl.uniform_1_i32(self.u_y.as_ref(), 1);
            gl.uniform_1_i32(self.u_uv.as_ref(), 2);
            gl.uniform_matrix_3_f32_slice(self.u_m.as_ref(), false, &m);
            gl.uniform_3_f32(self.u_off.as_ref(), off[0], off[1], off[2]);
            gl.uniform_4_f32(self.u_tint.as_ref(), tint[0], tint[1], tint[2], tint[3]);
            gl.bind_vertex_array(Some(self.vao));
            gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
            gl.bind_vertex_array(None);
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.active_texture(glow::TEXTURE1);
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.active_texture(glow::TEXTURE0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// PC5: the planes' sizes and strides, odd widths and heights rounded
    /// up as ffmpeg writes them (318 by 181 came out of ffmpeg 8 as 86,496
    /// bytes a picture).
    #[test]
    fn the_planes_are_laid_out_as_ffmpeg_writes_them() {
        let even = Layout::new(1920, 1080);
        assert_eq!((even.chroma_width, even.chroma_height), (960, 540));
        assert_eq!(even.y_len(), 2_073_600);
        assert_eq!(even.uv_stride(), 1920);
        assert_eq!(even.len(), 3_110_400, "12 bits a pixel");
        let odd = Layout::new(318, 181);
        assert_eq!((odd.chroma_width, odd.chroma_height), (159, 91));
        assert_eq!(odd.len(), 86_496);
        let odd_width = Layout::new(3, 2);
        assert_eq!(odd_width.uv_stride(), 4, "two chroma pairs for 3 pixels");
        assert_eq!(odd_width.len(), 6 + 4);
        let one = Layout::new(1, 1);
        assert_eq!(one.len(), 3);
        let data = vec![0u8; odd.len()];
        let (y, uv) = odd.split(&data).unwrap();
        assert_eq!((y.len(), uv.len()), (57_558, 28_938));
        assert!(odd.split(&data[1..]).is_none(), "a short picture");
    }

    /// PC5: limited-range black, white and grey stay so; full range too;
    /// a colour lands where BT.601 and BT.709 each put it.
    #[test]
    fn colours_turn_to_rgb_as_the_stream_says() {
        let layout = Layout::new(2, 2);
        let picture = |y: u8, u: u8, v: u8| vec![y, y, y, y, u, v];
        let limited = Colour::default();
        let pixel = |data: Vec<u8>, colour| to_rgba(&layout, &data, colour)[..4].to_vec();
        // Within 2 of each: the samples are rounded to bytes both ways.
        let near =
            |got: Vec<u8>, want: [u8; 4]| got.iter().zip(want).all(|(g, w)| g.abs_diff(w) <= 2);
        assert_eq!(pixel(picture(16, 128, 128), limited), [0, 0, 0, 255]);
        assert_eq!(pixel(picture(235, 128, 128), limited), [255, 255, 255, 255]);
        assert_eq!(pixel(picture(126, 128, 128), limited), [128, 128, 128, 255]);
        let full = Colour {
            full: true,
            ..Colour::default()
        };
        assert_eq!(pixel(picture(0, 128, 128), full), [0, 0, 0, 255]);
        assert_eq!(pixel(picture(255, 128, 128), full), [255, 255, 255, 255]);
        // Pure red as each matrix encodes it (limited range).
        let red = pixel(picture(81, 90, 240), limited);
        assert!(near(red.clone(), [255, 0, 0, 255]), "{red:?}");
        let bt709 = Colour {
            bt709: true,
            ..Colour::default()
        };
        let red = pixel(picture(63, 102, 240), bt709);
        assert!(near(red.clone(), [255, 0, 0, 255]), "{red:?}");
        // Read with the other matrix, it is not red.
        let wrong = pixel(picture(63, 102, 240), limited);
        assert!(!near(wrong.clone(), [255, 0, 0, 255]), "{wrong:?}");
        // The wrong size gives nothing rather than a misread picture.
        assert!(to_rgba(&layout, &[0; 5], limited).is_empty());
    }

    /// PC5: NV12 only where glow draws it and its shader has not failed;
    /// RGBA otherwise, as before.
    #[test]
    fn the_planes_fall_back_to_rgba_without_glow_or_its_shader() {
        assert_eq!(choose(true, false), Planes::Nv12);
        assert_eq!(choose(false, false), Planes::Rgba, "another renderer");
        assert_eq!(choose(true, true), Planes::Rgba, "the shader failed");
        assert_eq!(choose(false, true), Planes::Rgba);
        // The matrix GLSL reads column by column: BT.601 limited's white
        // (235, 128, 128) gives 1 in each.
        let (m, off) = coefficients(Colour::default());
        let s = [235.0 / 255.0 - off[0], 0.0, 0.0];
        for row in 0..3 {
            let v = m[row] * s[0] + m[3 + row] * s[1] + m[6 + row] * s[2];
            assert!((v - 1.0).abs() < 1e-4, "{row}: {v}");
        }
    }

    /// PC5: an odd width and height take the last chroma pair for the
    /// last column and row.
    #[test]
    fn an_odd_picture_reads_its_last_chroma() {
        let layout = Layout::new(3, 3);
        // Luma 126 everywhere; chroma: grey, then red-ish in the second
        // column pair and the second row.
        let mut data = vec![126u8; 9];
        data.extend_from_slice(&[128, 128, 90, 240, 128, 128, 90, 240]);
        let rgba = to_rgba(&layout, &data, Colour::default());
        assert_eq!(rgba.len(), 36);
        let at = |x: usize, y: usize| &rgba[(y * 3 + x) * 4..][..3];
        assert_eq!(at(0, 0), [128, 128, 128]);
        assert!(at(2, 0)[0] > 200 && at(2, 0)[2] < 100, "{:?}", at(2, 0));
        assert!(at(2, 2)[0] > 200, "the last row reads the last chroma row");
    }

    /// PC5: the shader draws an NV12 picture as its RGB, in the app's own
    /// renderer: a small window opens for a moment, and in a paint
    /// callback the picture (red then blue along its top half, blue then
    /// red along its bottom half, BT.601 limited) is drawn into a
    /// framebuffer of its own and read back: colours, and the top row at
    /// the top.
    /// Ignored: it opens a window. Windows only (winit's any-thread hook).
    #[cfg(windows)]
    #[test]
    #[ignore = "opens a window with OpenGL"]
    fn the_shader_draws_nv12_as_its_rgb() {
        use winit::platform::windows::EventLoopBuilderExtWindows as _;
        let (tx, rx) = std::sync::mpsc::channel::<Result<Vec<u8>, String>>();
        let tx = Mutex::new(Some(tx));
        let picture = Arc::new(Picture {
            width: 4,
            height: 4,
            planes: Planes::Nv12,
            // Luma: red's 81, blue's 41; chroma: red, blue, then blue, red.
            data: vec![
                81, 81, 41, 41, 81, 81, 41, 41, 41, 41, 81, 81, 41, 41, 81, 81, 90, 240, 240, 110,
                240, 110, 90, 240,
            ],
            colour: Colour::default(),
        });
        let (w, h) = (64i32, 32i32);
        let options = eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default().with_inner_size([160.0, 90.0]),
            renderer: eframe::Renderer::Glow,
            event_loop_builder: Some(Box::new(|builder| {
                builder.with_any_thread(true);
            })),
            ..Default::default()
        };
        let callback = Arc::new(egui_glow::CallbackFn::new(move |_info, painter| {
            let gl = painter.gl();
            // SAFETY: egui_glow's callback, its context current.
            let read = unsafe {
                Drawn::new(gl).and_then(|mut drawn| {
                    let target = gl.create_texture()?;
                    gl.bind_texture(glow::TEXTURE_2D, Some(target));
                    gl.tex_image_2d(
                        glow::TEXTURE_2D,
                        0,
                        glow::RGBA8 as i32,
                        w,
                        h,
                        0,
                        glow::RGBA,
                        glow::UNSIGNED_BYTE,
                        glow::PixelUnpackData::Slice(None),
                    );
                    let fbo = gl.create_framebuffer()?;
                    gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
                    gl.framebuffer_texture_2d(
                        glow::FRAMEBUFFER,
                        glow::COLOR_ATTACHMENT0,
                        glow::TEXTURE_2D,
                        Some(target),
                        0,
                    );
                    gl.disable(glow::SCISSOR_TEST);
                    gl.disable(glow::BLEND);
                    gl.viewport(0, 0, w, h);
                    drawn.upload(gl, &picture);
                    drawn.draw(gl, [1.0; 4]);
                    let mut pixels = vec![0u8; (w * h * 4) as usize];
                    gl.read_pixels(
                        0,
                        0,
                        w,
                        h,
                        glow::RGBA,
                        glow::UNSIGNED_BYTE,
                        glow::PixelPackData::Slice(Some(&mut pixels)),
                    );
                    gl.bind_framebuffer(glow::FRAMEBUFFER, None);
                    gl.delete_framebuffer(fbo);
                    gl.delete_texture(target);
                    Ok(pixels)
                })
            };
            if let Some(tx) = tx.lock().unwrap().take() {
                let _ = tx.send(read);
            }
        }));
        let _ = eframe::run_native(
            "pocketdesk-nv12-test",
            options,
            Box::new(move |_cc| {
                Ok(Box::new(Probe {
                    callback,
                    frames: 0,
                }))
            }),
        );
        let pixels = rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("the callback ran")
            .expect("the shader compiled and drew");
        // Rows come bottom first: the picture's top half is the upper.
        let at = |x: i32, y_up: i32| {
            let i = ((y_up * w + x) * 4) as usize;
            [pixels[i], pixels[i + 1], pixels[i + 2]]
        };
        let near =
            |got: [u8; 3], want: [u8; 3]| got.iter().zip(want).all(|(g, w)| g.abs_diff(w) <= 3);
        let (top, bottom) = (h * 7 / 8, h / 8);
        assert!(near(at(8, top), [255, 0, 0]), "top left: {:?}", at(8, top));
        assert!(
            near(at(56, top), [0, 0, 255]),
            "top right: {:?}",
            at(56, top)
        );
        assert!(
            near(at(8, bottom), [0, 0, 255]),
            "bottom left: {:?}",
            at(8, bottom)
        );
        assert!(
            near(at(56, bottom), [255, 0, 0]),
            "bottom right: {:?}",
            at(56, bottom)
        );
    }

    /// The test's app: its paint callback runs once, then it closes.
    #[cfg(windows)]
    struct Probe {
        callback: Arc<egui_glow::CallbackFn>,
        frames: u32,
    }

    #[cfg(windows)]
    impl eframe::App for Probe {
        fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
            let rect = ui.max_rect();
            ui.painter().add(egui::PaintCallback {
                rect,
                callback: self.callback.clone(),
            });
            self.frames += 1;
            if self.frames > 3 {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            }
            ui.ctx().request_repaint();
        }
    }
}
