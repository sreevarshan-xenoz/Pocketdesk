//! A full-screen window that shows the moving test pattern, a frame for every
//! frame the compositor asks for: a picture source for measuring the capture
//! (`core/tests/zero_copy_live.rs`) in a headless sway of its own, where
//! nothing else moves at the display's rate.
//!
//! `PD_SOURCE_SECS` (the default 60) is how long it runs, and
//! `POCKETDESK_TEST_PATTERN_TEXT=1` puts a page of text under the pattern, as
//! the latency probe does. It draws through shared memory and needs no GPU.
//! Run it only in a compositor of your own.
#![cfg(all(target_os = "linux", feature = "capture"))]

use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};
use std::time::{Duration, Instant};

use pocketdesk_core::streaming::test_pattern;
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_buffer::{self, WlBuffer};
use wayland_client::protocol::wl_callback::{self, WlCallback};
use wayland_client::protocol::wl_compositor::WlCompositor;
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::protocol::wl_shm::{self, WlShm};
use wayland_client::protocol::wl_shm_pool::WlShmPool;
use wayland_client::protocol::wl_surface::{self, WlSurface};
use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols::xdg::shell::client::xdg_surface::{self, XdgSurface};
use wayland_protocols::xdg::shell::client::xdg_toplevel::{self, XdgToplevel};
use wayland_protocols::xdg::shell::client::xdg_wm_base::{self, XdgWmBase};

struct App {
    surface: WlSurface,
    buffers: Vec<WlBuffer>,
    free: Vec<bool>,
    memory: *mut u8,
    buffer_len: usize,
    size: (u32, u32),
    configured: bool,
    frame_due: bool,
    started: bool,
    frame: u64,
    done: bool,
}

impl App {
    fn draw(&mut self, qh: &QueueHandle<App>) {
        let Some(at) = self.free.iter().position(|f| *f) else {
            return;
        };
        let (w, h) = self.size;
        let picture = test_pattern::frame(w, h, self.frame);
        self.frame += 1;
        let len = (w * h * 4) as usize;
        // SAFETY: the mapping holds two buffers of `buffer_len` bytes; `at`
        // is one of them, and the compositor is not reading it (it is free).
        let target = unsafe {
            std::slice::from_raw_parts_mut(self.memory.add(at * self.buffer_len), self.buffer_len)
        };
        target[..len.min(picture.data.len())]
            .copy_from_slice(&picture.data[..len.min(picture.data.len())]);
        self.free[at] = false;
        self.surface.frame(qh, ());
        self.surface.attach(Some(&self.buffers[at]), 0, 0);
        self.surface.damage_buffer(0, 0, w as i32, h as i32);
        self.surface.commit();
        self.frame_due = false;
    }
}

impl Dispatch<WlRegistry, GlobalListContents> for App {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: wayland_client::protocol::wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
impl Dispatch<WlCompositor, ()> for App {
    fn event(
        _: &mut Self,
        _: &WlCompositor,
        _: <WlCompositor as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
impl Dispatch<WlShm, ()> for App {
    fn event(
        _: &mut Self,
        _: &WlShm,
        _: wl_shm::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
impl Dispatch<WlShmPool, ()> for App {
    fn event(
        _: &mut Self,
        _: &WlShmPool,
        _: <WlShmPool as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
impl Dispatch<WlSurface, ()> for App {
    fn event(
        _: &mut Self,
        _: &WlSurface,
        _: wl_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
impl Dispatch<WlBuffer, usize> for App {
    fn event(
        state: &mut Self,
        _: &WlBuffer,
        event: wl_buffer::Event,
        at: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_buffer::Event::Release = event {
            state.free[*at] = true;
        }
    }
}
impl Dispatch<WlCallback, ()> for App {
    fn event(
        state: &mut Self,
        _: &WlCallback,
        event: wl_callback::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_callback::Event::Done { .. } = event {
            state.frame_due = true;
        }
    }
}
impl Dispatch<XdgWmBase, ()> for App {
    fn event(
        _: &mut Self,
        base: &XdgWmBase,
        event: xdg_wm_base::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_wm_base::Event::Ping { serial } = event {
            base.pong(serial);
        }
    }
}
impl Dispatch<XdgSurface, ()> for App {
    fn event(
        state: &mut Self,
        surface: &XdgSurface,
        event: xdg_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            surface.ack_configure(serial);
            state.configured = true;
        }
    }
}
impl Dispatch<XdgToplevel, ()> for App {
    fn event(
        state: &mut Self,
        _: &XdgToplevel,
        event: xdg_toplevel::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            xdg_toplevel::Event::Configure { width, height, .. } if width > 0 && height > 0 => {
                // The buffers are made for the first size; a later one keeps
                // them (the pattern is drawn into what fits).
                if !state.started {
                    state.size = (width as u32, height as u32);
                }
            }
            xdg_toplevel::Event::Close => state.done = true,
            _ => {}
        }
    }
}

fn main() {
    let secs: u64 = std::env::var("PD_SOURCE_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(60);
    let conn = Connection::connect_to_env().expect("a Wayland display");
    let (globals, mut queue) = registry_queue_init::<App>(&conn).expect("registry");
    let qh = queue.handle();
    let compositor: WlCompositor = globals.bind(&qh, 1..=4, ()).expect("wl_compositor");
    let shm: WlShm = globals.bind(&qh, 1..=1, ()).expect("wl_shm");
    let base: XdgWmBase = globals.bind(&qh, 1..=1, ()).expect("xdg_wm_base");
    let surface = compositor.create_surface(&qh, ());
    let xdg = base.get_xdg_surface(&surface, &qh, ());
    let toplevel = xdg.get_toplevel(&qh, ());
    toplevel.set_title("pd-source".into());
    toplevel.set_app_id("pd-source".into());
    toplevel.set_fullscreen(None);
    surface.commit();

    // The first configure names the size the buffers are made for.
    let mut app = App {
        surface: surface.clone(),
        buffers: Vec::new(),
        free: vec![true, true],
        memory: std::ptr::null_mut(),
        buffer_len: 0,
        size: (1920, 1080),
        configured: false,
        frame_due: true,
        started: false,
        frame: 0,
        done: false,
    };
    while !app.configured {
        queue.blocking_dispatch(&mut app).expect("dispatch");
    }
    let (w, h) = app.size;
    app.buffer_len = (w * h * 4) as usize;
    let total = app.buffer_len * 2;
    let name = std::ffi::CString::new("pd-source").unwrap();
    // SAFETY: memfd_create and ftruncate on the fresh descriptor; mmap of
    // exactly that length; all checked.
    let fd = unsafe { libc::memfd_create(name.as_ptr(), libc::MFD_CLOEXEC) };
    assert!(fd >= 0, "memfd_create");
    // SAFETY: `fd` is the new descriptor, owned from here.
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    // SAFETY: as above.
    assert_eq!(unsafe { libc::ftruncate(fd.as_raw_fd(), total as i64) }, 0);
    // SAFETY: a shared read-write mapping of the memfd's `total` bytes, kept
    // until the process ends.
    let memory = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            total,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED,
            fd.as_raw_fd(),
            0,
        )
    };
    assert_ne!(memory, libc::MAP_FAILED, "mmap");
    app.memory = memory as *mut u8;
    let pool = shm.create_pool(fd.as_fd(), total as i32, &qh, ());
    for at in 0..2usize {
        let buffer = pool.create_buffer(
            (at * app.buffer_len) as i32,
            w as i32,
            h as i32,
            (w * 4) as i32,
            wl_shm::Format::Xrgb8888,
            &qh,
            at,
        );
        app.buffers.push(buffer);
    }
    app.started = true;

    let until = Instant::now() + Duration::from_secs(secs);
    while !app.done && Instant::now() < until {
        if app.frame_due {
            app.draw(&qh);
        }
        // Waits for the compositor's next event (a frame callback, a buffer
        // release).
        let _ = queue.flush();
        if let Some(guard) = conn.prepare_read() {
            let mut pfd = libc::pollfd {
                fd: guard.connection_fd().as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: one valid pollfd, a bounded wait.
            unsafe { libc::poll(&mut pfd, 1, 100) };
            let _ = guard.read();
        }
        queue.dispatch_pending(&mut app).expect("dispatch");
    }
    toplevel.destroy();
    xdg.destroy();
    surface.destroy();
    let _ = queue.roundtrip(&mut app);
    println!("drew {} frames", app.frame);
}
