use std::ffi::{CStr, CString, c_char, c_int, c_ulong, c_void};
use std::io::Write;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use base64::Engine as _;

use crate::model::MediaKind;

pub const MAX_FPS: u32 = 30;
pub const MAX_PIXELS: u32 = 1280 * 720;
pub const SEEK_SECONDS: i32 = 5;
pub const VOLUME_STEP: i32 = 5;
const POLL: Duration = Duration::from_millis(100);
const STUCK: Duration = Duration::from_secs(3);
const LIBRARIES: [&str; 2] = ["libmpv.so.2", "libmpv.so"];

const RENDER_API_TYPE: c_int = 1;
const RENDER_SKIP: c_int = 13;
const RENDER_SW_SIZE: c_int = 17;
const RENDER_SW_FORMAT: c_int = 18;
const RENDER_SW_STRIDE: c_int = 19;
const RENDER_SW_POINTER: c_int = 20;
const UPDATE_FRAME: u64 = 1;
const FORMAT_FLAG: c_int = 3;
const FORMAT_INT64: c_int = 4;
const FORMAT_DOUBLE: c_int = 5;
const EVENT_NONE: c_int = 0;
const EVENT_SHUTDOWN: c_int = 1;
const EVENT_END_FILE: c_int = 7;
const END_FILE_ERROR: c_int = 4;

type Handle = *mut c_void;
type Callback = Option<unsafe extern "C" fn(*mut c_void)>;

#[repr(C)]
struct RenderParam {
    kind: c_int,
    data: *mut c_void,
}

#[repr(C)]
struct MpvEvent {
    event_id: c_int,
    error: c_int,
    reply_userdata: u64,
    data: *mut c_void,
}

#[repr(C)]
struct EndFile {
    reason: c_int,
    error: c_int,
}

struct Lib {
    _lib: libloading::Library,
    create: unsafe extern "C" fn() -> Handle,
    initialize: unsafe extern "C" fn(Handle) -> c_int,
    set_option_string: unsafe extern "C" fn(Handle, *const c_char, *const c_char) -> c_int,
    command: unsafe extern "C" fn(Handle, *mut *const c_char) -> c_int,
    command_string: unsafe extern "C" fn(Handle, *const c_char) -> c_int,
    get_property: unsafe extern "C" fn(Handle, *const c_char, c_int, *mut c_void) -> c_int,
    wait_event: unsafe extern "C" fn(Handle, f64) -> *mut MpvEvent,
    set_wakeup_callback: unsafe extern "C" fn(Handle, Callback, *mut c_void),
    terminate_destroy: unsafe extern "C" fn(Handle),
    error_string: unsafe extern "C" fn(c_int) -> *const c_char,
    render_create: unsafe extern "C" fn(*mut Handle, Handle, *mut RenderParam) -> c_int,
    render_set_update_callback: unsafe extern "C" fn(Handle, Callback, *mut c_void),
    render_update: unsafe extern "C" fn(Handle) -> u64,
    render_render: unsafe extern "C" fn(Handle, *mut RenderParam) -> c_int,
    render_free: unsafe extern "C" fn(Handle),
}

fn load(names: &[&str]) -> Result<Lib, String> {
    let mut errors = Vec::new();
    for name in names {
        match unsafe { libloading::Library::new(name) } {
            Ok(lib) => return bind(lib).map_err(|e| format!("{name}: {e}")),
            Err(e) => errors.push(e.to_string()),
        }
    }
    Err(format!("libmpv not found ({})", errors.join("; ")))
}

fn bind(lib: libloading::Library) -> Result<Lib, libloading::Error> {
    unsafe {
        let version: unsafe extern "C" fn() -> c_ulong = *lib.get(b"mpv_client_api_version\0")?;
        let major = version() >> 16;
        if major != 2 {
            return Err(libloading::Error::DlSymUnknown);
        }
        Ok(Lib {
            create: *lib.get(b"mpv_create\0")?,
            initialize: *lib.get(b"mpv_initialize\0")?,
            set_option_string: *lib.get(b"mpv_set_option_string\0")?,
            command: *lib.get(b"mpv_command\0")?,
            command_string: *lib.get(b"mpv_command_string\0")?,
            get_property: *lib.get(b"mpv_get_property\0")?,
            wait_event: *lib.get(b"mpv_wait_event\0")?,
            set_wakeup_callback: *lib.get(b"mpv_set_wakeup_callback\0")?,
            terminate_destroy: *lib.get(b"mpv_terminate_destroy\0")?,
            error_string: *lib.get(b"mpv_error_string\0")?,
            render_create: *lib.get(b"mpv_render_context_create\0")?,
            render_set_update_callback: *lib.get(b"mpv_render_context_set_update_callback\0")?,
            render_update: *lib.get(b"mpv_render_context_update\0")?,
            render_render: *lib.get(b"mpv_render_context_render\0")?,
            render_free: *lib.get(b"mpv_render_context_free\0")?,
            _lib: lib,
        })
    }
}

fn library() -> Result<&'static Lib, String> {
    static LIB: OnceLock<Result<Lib, String>> = OnceLock::new();
    LIB.get_or_init(|| load(&LIBRARIES))
        .as_ref()
        .map_err(Clone::clone)
}

pub fn probe() -> Result<(), String> {
    library().map(|_| ())
}

struct Mpv {
    lib: &'static Lib,
    handle: Handle,
    render: Handle,
}

impl Mpv {
    fn create(lib: &'static Lib) -> Result<Self, String> {
        let handle = unsafe { (lib.create)() };
        if handle.is_null() {
            return Err("libmpv could not be created".to_owned());
        }
        Ok(Self {
            lib,
            handle,
            render: std::ptr::null_mut(),
        })
    }

    fn error(&self, code: c_int) -> String {
        let s = unsafe { (self.lib.error_string)(code) };
        if s.is_null() {
            return format!("error {code}");
        }
        unsafe { CStr::from_ptr(s) }.to_string_lossy().into_owned()
    }

    fn check(&self, what: &str, code: c_int) -> Result<(), String> {
        if code < 0 {
            Err(format!("{what}: {}", self.error(code)))
        } else {
            Ok(())
        }
    }

    fn option(&self, name: &str, value: &str) -> Result<(), String> {
        let (n, v) = (cstring(name)?, cstring(value)?);
        let code = unsafe { (self.lib.set_option_string)(self.handle, n.as_ptr(), v.as_ptr()) };
        self.check(&format!("mpv option {name}={value}"), code)
    }

    fn command_string(&self, cmd: &str) -> Result<(), String> {
        let c = cstring(cmd)?;
        let code = unsafe { (self.lib.command_string)(self.handle, c.as_ptr()) };
        self.check(cmd, code)
    }

    fn loadfile(&self, path: &str) -> Result<(), String> {
        let args = [cstring("loadfile")?, cstring(path)?];
        let mut argv: Vec<*const c_char> = args.iter().map(|a| a.as_ptr()).collect();
        argv.push(std::ptr::null());
        let code = unsafe { (self.lib.command)(self.handle, argv.as_mut_ptr()) };
        self.check("loadfile", code)
    }

    fn get<T: Default>(&self, name: &CStr, format: c_int) -> Option<T> {
        let mut out = T::default();
        let code = unsafe {
            (self.lib.get_property)(
                self.handle,
                name.as_ptr(),
                format,
                (&raw mut out).cast::<c_void>(),
            )
        };
        (code >= 0).then_some(out)
    }

    fn double(&self, name: &CStr) -> Option<f64> {
        self.get::<f64>(name, FORMAT_DOUBLE)
    }

    fn flag(&self, name: &CStr) -> Option<bool> {
        self.get::<c_int>(name, FORMAT_FLAG).map(|v| v != 0)
    }

    fn int(&self, name: &CStr) -> Option<i64> {
        self.get::<i64>(name, FORMAT_INT64)
    }
}

impl Drop for Mpv {
    fn drop(&mut self) {
        unsafe {
            if !self.render.is_null() {
                (self.lib.render_free)(self.render);
            }
            (self.lib.terminate_destroy)(self.handle);
        }
    }
}

fn cstring(s: &str) -> Result<CString, String> {
    CString::new(s).map_err(|_| format!("NUL byte in {s:?}"))
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Status {
    pub loaded: bool,
    pub pos: f64,
    pub duration: f64,
    pub paused: bool,
    pub volume: f64,
    pub muted: bool,
    pub eof: bool,
    pub dims: Option<(u32, u32)>,
    pub fps: f64,
    pub error: Option<String>,
    pub ended: bool,
}

impl Status {
    fn shown(&self) -> Self {
        Self {
            pos: self.pos.trunc(),
            duration: self.duration.trunc(),
            volume: self.volume.round(),
            fps: 0.0,
            ..self.clone()
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Place {
    pub cols: u16,
    pub rows: u16,
    pub w: u32,
    pub h: u32,
}

pub struct Frame {
    pub place: Place,
    pub data: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct Options {
    pub audio_out: Option<String>,
    pub video: bool,
    pub looped: bool,
    pub round: bool,
    pub volume: f64,
    pub muted: bool,
}

impl Options {
    pub fn for_kind(kind: MediaKind, volume: f64, muted: bool) -> Self {
        Self {
            audio_out: None,
            video: !matches!(kind, MediaKind::Voice | MediaKind::Audio),
            looped: matches!(kind, MediaKind::VideoNote | MediaKind::Animation),
            round: kind == MediaKind::VideoNote,
            volume,
            muted,
        }
    }

    fn mpv(&self) -> Vec<(&'static str, String)> {
        let mut out: Vec<(&'static str, String)> = [
            ("vo", "libmpv"),
            ("config", "no"),
            ("load-scripts", "no"),
            ("ytdl", "no"),
            ("osc", "no"),
            ("load-stats-overlay", "no"),
            ("load-console", "no"),
            ("load-select", "no"),
            ("load-positioning", "no"),
            ("load-commands", "no"),
            ("load-context-menu", "no"),
            ("load-auto-profiles", "no"),
            ("input-default-bindings", "no"),
            ("input-vo-keyboard", "no"),
            ("terminal", "no"),
            ("sub-auto", "no"),
            ("audio-display", "no"),
            ("keep-open", "yes"),
            ("idle", "yes"),
            ("audio-client-name", "gramtty"),
        ]
        .into_iter()
        .map(|(k, v)| (k, v.to_owned()))
        .collect();
        if let Some(ao) = &self.audio_out {
            out.push(("ao", ao.clone()));
        }
        if !self.video {
            out.push(("vid", "no".to_owned()));
        }
        out.push((
            "loop-file",
            if self.looped { "inf" } else { "no" }.to_owned(),
        ));
        out.push((
            "volume",
            format!("{}", self.volume.clamp(0.0, 100.0).round()),
        ));
        out.push(("mute", if self.muted { "yes" } else { "no" }.to_owned()));
        out
    }
}

#[derive(Default)]
struct Inner {
    frame: Option<Frame>,
    spare: Option<Vec<u8>>,
    status: Status,
    cmds: Vec<String>,
    want: Place,
    quit: bool,
    render: bool,
    events: bool,
}

struct Shared {
    inner: Mutex<Inner>,
    cv: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn poke(&self, f: impl FnOnce(&mut Inner)) {
        f(&mut self.lock());
        self.cv.notify_one();
    }
}

unsafe extern "C" fn on_render_update(ctx: *mut c_void) {
    let shared = unsafe { &*ctx.cast::<Shared>() };
    shared.poke(|i| i.render = true);
}

unsafe extern "C" fn on_wakeup(ctx: *mut c_void) {
    let shared = unsafe { &*ctx.cast::<Shared>() };
    shared.poke(|i| i.events = true);
}

pub struct Player {
    shared: Arc<Shared>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Player {
    pub fn start(
        path: &str,
        opts: Options,
        notify: impl Fn() + Send + 'static,
    ) -> Result<Self, String> {
        let lib = library()?;
        let shared = Arc::new(Shared {
            inner: Mutex::new(Inner::default()),
            cv: Condvar::new(),
        });
        let worker = shared.clone();
        let path = path.to_owned();
        let thread = std::thread::Builder::new()
            .name("gramtty-video".to_owned())
            .spawn(move || {
                if let Err(e) = run(lib, &worker, &path, &opts, &notify) {
                    worker.lock().status.error = Some(e);
                }
                worker.lock().status.ended = true;
                notify();
            })
            .map_err(|e| format!("video thread: {e}"))?;
        Ok(Self {
            shared,
            thread: Some(thread),
        })
    }

    pub fn command(&self, cmd: impl Into<String>) {
        let cmd = cmd.into();
        self.shared.poke(|i| i.cmds.push(cmd));
    }

    pub fn want(&self, place: Place) {
        let mut g = self.shared.lock();
        if g.want != place {
            g.want = place;
            g.render = true;
            drop(g);
            self.shared.cv.notify_one();
        }
    }

    pub fn take_frame(&self) -> Option<Frame> {
        self.shared.lock().frame.take()
    }

    pub fn recycle(&self, data: Vec<u8>) {
        self.shared.lock().spare = Some(data);
    }

    pub fn status(&self) -> Status {
        self.shared.lock().status.clone()
    }

    #[cfg(test)]
    pub fn join(mut self) {
        self.shared.poke(|i| i.quit = true);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.shared.poke(|i| i.quit = true);
        drop(self.thread.take());
    }
}

fn run(
    lib: &'static Lib,
    shared: &Arc<Shared>,
    path: &str,
    opts: &Options,
    notify: &dyn Fn(),
) -> Result<(), String> {
    let mut mpv = Mpv::create(lib)?;
    for (k, v) in opts.mpv() {
        mpv.option(k, &v)?;
    }
    mpv.check("mpv init", unsafe { (lib.initialize)(mpv.handle) })?;
    let ctx = Arc::as_ptr(shared).cast_mut().cast::<c_void>();
    let mut api = *b"sw\0";
    let mut params = [
        RenderParam {
            kind: RENDER_API_TYPE,
            data: api.as_mut_ptr().cast(),
        },
        RenderParam {
            kind: 0,
            data: std::ptr::null_mut(),
        },
    ];
    let code = unsafe { (lib.render_create)(&raw mut mpv.render, mpv.handle, params.as_mut_ptr()) };
    mpv.check("mpv render context", code)?;
    unsafe {
        (lib.render_set_update_callback)(mpv.render, Some(on_render_update), ctx);
        (lib.set_wakeup_callback)(mpv.handle, Some(on_wakeup), ctx);
    }
    mpv.loadfile(path)?;
    let mut counter: u64 = 0;
    let mut drawn = Place::default();
    let mut seen = false;
    let mut last_poll: Option<Instant> = None;
    let mut buf: Vec<u8> = Vec::new();
    let mut shown = shared.lock().status.shown();
    loop {
        let (cmds, render, events, want) = {
            let mut g = shared.lock();
            let wait = POLL.saturating_sub(last_poll.map_or(POLL, |t| t.elapsed()));
            if !g.quit && !g.render && !g.events && g.cmds.is_empty() && !wait.is_zero() {
                g = shared
                    .cv
                    .wait_timeout(g, wait)
                    .unwrap_or_else(PoisonError::into_inner)
                    .0;
            }
            if g.quit {
                return Ok(());
            }
            let render = std::mem::take(&mut g.render);
            let events = std::mem::take(&mut g.events);
            (std::mem::take(&mut g.cmds), render, events, g.want)
        };
        for cmd in &cmds {
            let _ = mpv.command_string(cmd);
        }
        let mut failure = None;
        if events {
            loop {
                let ev = unsafe { (lib.wait_event)(mpv.handle, 0.0) };
                let Some(ev) = (unsafe { ev.as_ref() }) else {
                    break;
                };
                match ev.event_id {
                    EVENT_NONE => break,
                    EVENT_SHUTDOWN => return Ok(()),
                    EVENT_END_FILE if !ev.data.is_null() => {
                        let end = unsafe { &*ev.data.cast::<EndFile>() };
                        if end.reason == END_FILE_ERROR {
                            failure =
                                Some(format!("cannot play this file: {}", mpv.error(end.error)));
                        }
                    }
                    _ => {}
                }
            }
        }
        if let Some(e) = failure {
            return Err(e);
        }
        if events || !cmds.is_empty() || last_poll.is_none_or(|t| t.elapsed() >= POLL) {
            last_poll = Some(Instant::now());
            let prev = shared.lock().status.clone();
            let status = poll(&mpv, &prev);
            let now = status.shown();
            shared.lock().status = status;
            if now != shown {
                shown = now;
                notify();
            }
        }
        if render {
            let flags = unsafe { (lib.render_update)(mpv.render) };
            let fresh = flags & UPDATE_FRAME != 0;
            seen |= fresh;
            let resized = seen && want != drawn;
            if fresh || resized {
                let every = frame_step(shared.lock().status.fps);
                let due = resized || (fresh && counter.is_multiple_of(every));
                counter += u64::from(fresh);
                let draw = due && want.w > 0 && want.h > 0;
                if draw && let Some(spare) = shared.lock().spare.take() {
                    buf = spare;
                }
                render_frame(&mpv, &mut buf, want, !draw)?;
                if draw {
                    drawn = want;
                    if opts.round {
                        round_mask(&mut buf, want.w, want.h);
                    } else {
                        opaque(&mut buf);
                    }
                    let data = std::mem::take(&mut buf);
                    let mut g = shared.lock();
                    let first = g.frame.is_none();
                    if let Some(old) = g.frame.replace(Frame { place: want, data }) {
                        buf = old.data;
                    }
                    drop(g);
                    if first {
                        notify();
                    }
                }
            }
        }
    }
}

fn poll(mpv: &Mpv, prev: &Status) -> Status {
    let dims = match (mpv.int(c"dwidth"), mpv.int(c"dheight")) {
        (Some(w), Some(h)) if w > 0 && h > 0 => Some((w as u32, h as u32)),
        _ => prev.dims,
    };
    let duration = mpv.double(c"duration");
    Status {
        loaded: duration.is_some() || prev.loaded,
        pos: mpv.double(c"time-pos").unwrap_or(prev.pos),
        duration: duration.unwrap_or(prev.duration),
        paused: mpv.flag(c"pause").unwrap_or(prev.paused),
        volume: mpv.double(c"volume").unwrap_or(prev.volume),
        muted: mpv.flag(c"mute").unwrap_or(prev.muted),
        eof: mpv.flag(c"eof-reached").unwrap_or(false),
        dims,
        fps: mpv
            .double(c"estimated-vf-fps")
            .or_else(|| mpv.double(c"container-fps"))
            .unwrap_or(prev.fps),
        error: None,
        ended: false,
    }
}

fn render_frame(mpv: &Mpv, buf: &mut Vec<u8>, want: Place, skip: bool) -> Result<(), String> {
    let (w, h) = if skip { (16, 16) } else { (want.w, want.h) };
    let mut size: [c_int; 2] = [w as c_int, h as c_int];
    let mut stride: usize = w as usize * 4;
    let len = stride * h as usize;
    if buf.len() != len {
        buf.resize(len, 0);
    }
    let mut format = *b"rgb0\0";
    let mut skip_flag: c_int = c_int::from(skip);
    let mut params = [
        RenderParam {
            kind: RENDER_SW_SIZE,
            data: size.as_mut_ptr().cast(),
        },
        RenderParam {
            kind: RENDER_SW_FORMAT,
            data: format.as_mut_ptr().cast(),
        },
        RenderParam {
            kind: RENDER_SW_STRIDE,
            data: (&raw mut stride).cast(),
        },
        RenderParam {
            kind: RENDER_SW_POINTER,
            data: buf.as_mut_ptr().cast(),
        },
        RenderParam {
            kind: RENDER_SKIP,
            data: (&raw mut skip_flag).cast(),
        },
        RenderParam {
            kind: 0,
            data: std::ptr::null_mut(),
        },
    ];
    let code = unsafe { (mpv.lib.render_render)(mpv.render, params.as_mut_ptr()) };
    mpv.check("mpv render", code)
}

pub fn frame_step(fps: f64) -> u64 {
    if fps.is_finite() && fps > 0.0 {
        ((fps / f64::from(MAX_FPS) - 0.05).ceil() as u64).max(1)
    } else {
        1
    }
}

pub fn opaque(buf: &mut [u8]) {
    for px in buf.as_chunks_mut::<4>().0 {
        px[3] = 255;
    }
}

pub fn round_mask(buf: &mut [u8], w: u32, h: u32) {
    let r = f64::from(w.min(h)) / 2.0;
    let (cx, cy) = (f64::from(w) / 2.0, f64::from(h) / 2.0);
    for (y, row) in buf.chunks_exact_mut(w as usize * 4).enumerate() {
        let dy = y as f64 + 0.5 - cy;
        for (x, px) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let dx = x as f64 + 0.5 - cx;
            let edge = r - (dx * dx + dy * dy).sqrt();
            px[3] = (edge.clamp(0.0, 1.0) * 255.0).round() as u8;
        }
    }
}

pub fn fit_pixels(dims: (u32, u32), cells: (u16, u16), font: (u16, u16)) -> (u32, u32) {
    let (dw, dh) = (f64::from(dims.0.max(1)), f64::from(dims.1.max(1)));
    let bw = f64::from(cells.0) * f64::from(font.0.max(1));
    let bh = f64::from(cells.1) * f64::from(font.1.max(1));
    let cap = (f64::from(MAX_PIXELS) / (dw * dh)).sqrt();
    let scale = (bw / dw).min(bh / dh).min(1.0).min(cap);
    let even = |v: f64| ((v.round() as u32) & !1).max(2);
    (even(dw * scale), even(dh * scale))
}

pub fn playable(kind: MediaKind) -> bool {
    matches!(
        kind,
        MediaKind::Video
            | MediaKind::VideoNote
            | MediaKind::Animation
            | MediaKind::Voice
            | MediaKind::Audio
    )
}

pub fn progress_line(st: &Status, width: usize) -> String {
    let icon = if st.eof && !st.paused {
        "■"
    } else if st.paused || st.eof {
        "⏸"
    } else {
        "▶"
    };
    let now = crate::format::duration(st.pos as i32);
    let total = crate::format::duration(st.duration as i32);
    let sound = if st.muted {
        "muted".to_owned()
    } else {
        format!("vol {}%", st.volume.round() as i64)
    };
    let fixed = format!(" {icon} {now}  {total}  {sound} ");
    let bar_w = width.saturating_sub(fixed.chars().count());
    let filled = if st.duration > 0.0 {
        ((st.pos / st.duration).clamp(0.0, 1.0) * bar_w as f64).round() as usize
    } else {
        0
    };
    let bar = format!("{}{}", "━".repeat(filled), "─".repeat(bar_w - filled));
    format!(" {icon} {now} {bar} {total}  {sound} ")
}

pub struct Inline {
    enabled: bool,
    pub audio_out: Option<String>,
    active: Option<Active>,
    pub requested: Option<(i64, usize)>,
    volume: f64,
    muted: bool,
    cmds: Vec<String>,
    seq: u64,
    error: Option<String>,
}

struct Active {
    target: (i64, usize),
    player: Player,
    id: u32,
    want: Place,
    placed: Option<Place>,
    frame: Option<Frame>,
    inflight: Option<(String, Instant)>,
    status: Status,
}

impl Default for Inline {
    fn default() -> Self {
        Self {
            enabled: false,
            audio_out: None,
            active: None,
            requested: None,
            volume: 100.0,
            muted: false,
            cmds: Vec::new(),
            seq: 0,
            error: None,
        }
    }
}

impl Inline {
    pub fn enable(&mut self) {
        self.enabled = true;
    }

    pub fn available(&self) -> Result<(), String> {
        if !self.enabled {
            return Err("inline playback needs kitty".to_owned());
        }
        probe()
    }

    pub fn active(&self) -> Option<(i64, usize)> {
        self.active.as_ref().map(|a| a.target)
    }

    pub fn play(
        &mut self,
        target: (i64, usize),
        kind: MediaKind,
        path: &str,
        id: u32,
        notify: impl Fn() + Send + 'static,
    ) -> Result<(), String> {
        self.stop();
        let mut opts = Options::for_kind(kind, self.volume, self.muted);
        opts.audio_out.clone_from(&self.audio_out);
        let player = Player::start(path, opts, notify)?;
        self.active = Some(Active {
            target,
            player,
            id,
            want: Place::default(),
            placed: None,
            frame: None,
            inflight: None,
            status: Status::default(),
        });
        Ok(())
    }

    pub fn stop(&mut self) {
        let Some(a) = self.active.take() else {
            return;
        };
        if a.placed.is_some() {
            self.cmds.push(crate::anim::delete(a.id));
        }
        if let Some((name, _)) = a.inflight {
            let _ = rustix::shm::unlink(name.as_str());
        }
    }

    pub fn command(&mut self, cmd: &str) {
        if let Some(a) = &self.active {
            a.player.command(cmd);
        }
    }

    pub fn toggle_pause(&mut self) {
        if let Some(a) = &self.active {
            if a.status.eof {
                a.player.command("seek 0 absolute");
                a.player.command("set pause no");
            } else {
                a.player.command("cycle pause");
            }
        }
    }

    pub fn status(&self) -> Option<&Status> {
        self.active.as_ref().map(|a| &a.status)
    }

    pub fn image(&self) -> Option<u32> {
        self.active
            .as_ref()
            .filter(|a| a.placed.is_some())
            .map(|a| a.id)
    }

    pub fn want(&mut self, place: Place) {
        if let Some(a) = &mut self.active
            && a.want != place
        {
            a.want = place;
            a.player.want(place);
        }
    }

    pub fn sync(&mut self) -> Option<String> {
        if let Some(e) = self.error.take() {
            self.stop();
            return Some(e);
        }
        let a = self.active.as_mut()?;
        a.status = a.player.status();
        if !a.status.ended {
            if a.status.loaded {
                self.volume = a.status.volume;
                self.muted = a.status.muted;
            }
            return None;
        }
        let err = a.status.error.clone();
        self.stop();
        err
    }

    pub fn waiting(&self) -> bool {
        self.active
            .as_ref()
            .is_some_and(|a| a.frame.is_some() && a.inflight.is_some())
    }

    pub fn flush(&mut self, w: &mut impl Write) -> std::io::Result<()> {
        for cmd in std::mem::take(&mut self.cmds) {
            w.write_all(cmd.as_bytes())?;
        }
        let Some(a) = self.active.as_mut() else {
            return Ok(());
        };
        if let Some(f) = a.player.take_frame()
            && let Some(old) = a.frame.replace(f)
        {
            a.player.recycle(old.data);
        }
        if a.frame.is_none() {
            return Ok(());
        }
        if let Some((name, since)) = &a.inflight {
            if shm_exists(name) {
                if since.elapsed() > STUCK {
                    self.error = Some(
                        "the terminal does not read video frames from shared memory; x opens mpv"
                            .to_owned(),
                    );
                }
                return Ok(());
            }
            a.inflight = None;
        }
        let Some(frame) = a.frame.take() else {
            return Ok(());
        };
        self.seq += 1;
        let name = format!("/gramtty-video-{}-{}", std::process::id(), self.seq);
        if let Err(e) = write_shm(&name, &frame.data) {
            self.error = Some(format!("video frame: {e}"));
            return Ok(());
        }
        let p = frame.place;
        let len = frame.data.len();
        let b64 = base64::engine::general_purpose::STANDARD.encode(name.as_bytes());
        if a.placed != Some(p) {
            if a.placed.is_some() {
                w.write_all(crate::anim::delete(a.id).as_bytes())?;
            }
            write!(
                w,
                "\x1b_Ga=T,U=1,i={},f=32,t=s,s={},v={},S={len},c={},r={},q=2;{b64}\x1b\\",
                a.id, p.w, p.h, p.cols, p.rows
            )?;
            a.placed = Some(p);
        } else {
            write!(
                w,
                "\x1b_Ga=f,r=1,i={},f=32,t=s,s={},v={},S={len},X=1,q=2;{b64}\x1b\\",
                a.id, p.w, p.h
            )?;
        }
        a.inflight = Some((name, Instant::now()));
        a.player.recycle(frame.data);
        Ok(())
    }
}

impl Drop for Inline {
    fn drop(&mut self) {
        self.stop();
    }
}

fn write_shm(name: &str, data: &[u8]) -> Result<(), String> {
    use rustix::fs::Mode;
    use rustix::shm::OFlags;
    let fd = rustix::shm::open(
        name,
        OFlags::CREATE | OFlags::EXCL | OFlags::RDWR,
        Mode::RUSR | Mode::WUSR,
    )
    .map_err(|e| format!("shm_open {name}: {e}"))?;
    let mut file = std::fs::File::from(fd);
    if let Err(e) = file.write_all(data) {
        let _ = rustix::shm::unlink(name);
        return Err(format!("write {name}: {e}"));
    }
    Ok(())
}

fn shm_exists(name: &str) -> bool {
    rustix::shm::open(name, rustix::shm::OFlags::RDONLY, rustix::fs::Mode::empty()).is_ok()
}

#[cfg(test)]
pub mod tests {
    use super::*;

    fn clip(name: &str, lavfi: &[&str], extra: &[&str]) -> Option<String> {
        if std::process::Command::new("ffmpeg")
            .arg("-version")
            .output()
            .is_err()
        {
            return None;
        }
        let dir = std::env::temp_dir().join(format!("gramtty-video-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join(name);
        let mut args: Vec<&str> = vec!["-v", "error", "-y"];
        for l in lavfi {
            args.extend(["-f", "lavfi", "-i", l]);
        }
        args.extend(extra);
        let out_s = out.to_string_lossy().into_owned();
        let status = std::process::Command::new("ffmpeg")
            .args(&args)
            .arg(&out_s)
            .status()
            .unwrap();
        assert!(status.success());
        Some(out_s)
    }

    pub fn test_video() -> Option<String> {
        clip(
            "v.mp4",
            &[
                "testsrc2=size=640x360:rate=30:duration=2",
                "sine=f=440:duration=2",
            ],
            &[
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
                "-c:a",
                "aac",
                "-shortest",
            ],
        )
    }

    fn wait_for(p: &Player, ok: impl Fn(&Status) -> bool) -> Status {
        let until = Instant::now() + Duration::from_secs(10);
        loop {
            let s = p.status();
            if ok(&s) || Instant::now() > until {
                return s;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn null_audio(kind: MediaKind) -> Options {
        let mut o = Options::for_kind(kind, 80.0, false);
        o.audio_out = Some("null".to_owned());
        o
    }

    #[test]
    fn fitting_keeps_aspect_caps_pixels_and_never_upscales() {
        assert_eq!(fit_pixels((1280, 720), (100, 40), (10, 21)), (1000, 562));
        assert_eq!(fit_pixels((320, 240), (100, 40), (10, 21)), (320, 240));
        let (w, h) = fit_pixels((3840, 2160), (400, 120), (10, 21));
        assert!(w * h <= MAX_PIXELS, "{w}x{h}");
        assert_eq!((w % 2, h % 2), (0, 0));
        assert_eq!(fit_pixels((0, 0), (1, 1), (0, 0)), (2, 2));
    }

    #[test]
    fn frame_rate_is_capped_by_dropping_whole_frames() {
        assert_eq!(frame_step(0.0), 1);
        assert_eq!(frame_step(29.97), 1);
        assert_eq!(frame_step(30.0), 1);
        assert_eq!(frame_step(30.000000000000018), 1);
        assert_eq!(frame_step(31.0), 1);
        assert_eq!(frame_step(50.0), 2);
        assert_eq!(frame_step(60.0), 2);
        assert_eq!(frame_step(120.0), 4);
        assert_eq!(frame_step(f64::NAN), 1);
    }

    #[test]
    fn round_mask_clears_corners_and_keeps_the_middle() {
        let mut buf = vec![9u8; 40 * 40 * 4];
        round_mask(&mut buf, 40, 40);
        assert_eq!(buf[3], 0);
        let mid = (20 * 40 + 20) * 4;
        assert_eq!(buf[mid + 3], 255);
        assert_eq!(&buf[mid..mid + 3], &[9, 9, 9]);
        let mut solid = vec![0u8; 8];
        opaque(&mut solid);
        assert_eq!(solid, [0, 0, 0, 255, 0, 0, 0, 255]);
    }

    #[test]
    fn progress_line_fills_the_width_and_shows_state() {
        let st = Status {
            pos: 30.0,
            duration: 60.0,
            volume: 80.0,
            ..Status::default()
        };
        let line = progress_line(&st, 50);
        assert_eq!(line.chars().count(), 50, "{line}");
        assert!(line.starts_with(" ▶ 0:30 ━"), "{line}");
        assert!(line.ends_with(" 1:00  vol 80% "), "{line}");
        let paused = progress_line(
            &Status {
                paused: true,
                muted: true,
                ..st
            },
            50,
        );
        assert!(paused.contains('⏸') && paused.contains("muted"), "{paused}");
        assert!(!progress_line(&Status::default(), 3).is_empty());
    }

    #[test]
    fn mpv_options_follow_the_media_kind() {
        let voice = Options::for_kind(MediaKind::Voice, 50.0, true).mpv();
        assert!(voice.contains(&("vid", "no".to_owned())));
        assert!(voice.contains(&("mute", "yes".to_owned())));
        assert!(voice.contains(&("loop-file", "no".to_owned())));
        let note = Options::for_kind(MediaKind::VideoNote, 100.0, false);
        assert!(note.round && note.looped && note.video);
        assert!(note.mpv().contains(&("vo", "libmpv".to_owned())));
        assert!(note.mpv().contains(&("config", "no".to_owned())));
        assert!(!note.mpv().iter().any(|(k, _)| *k == "ao"));
    }

    #[test]
    fn missing_library_is_reported_not_fatal() {
        let err = load(&["libmpv-does-not-exist.so.9"]).err().unwrap();
        assert!(err.starts_with("libmpv not found"), "{err}");
    }

    #[test]
    fn libmpv_renders_frames_at_the_requested_size_and_obeys_commands() {
        if probe().is_err() {
            eprintln!("libmpv is not installed; skipping");
            return;
        }
        let Some(path) = test_video() else {
            return;
        };
        let notified = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let n = notified.clone();
        let p = Player::start(&path, null_audio(MediaKind::Video), move || {
            n.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        })
        .unwrap();
        let st = wait_for(&p, |s| s.dims.is_some());
        assert_eq!(st.dims, Some((640, 360)), "{st:?}");
        assert!(st.error.is_none(), "{st:?}");
        let place = Place {
            cols: 32,
            rows: 9,
            w: 320,
            h: 180,
        };
        p.want(place);
        let until = Instant::now() + Duration::from_secs(10);
        let frame = loop {
            if let Some(f) = p.take_frame() {
                break f;
            }
            assert!(Instant::now() < until, "no frame");
            std::thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(frame.place, place);
        assert_eq!(frame.data.len(), 320 * 180 * 4);
        assert!(frame.data.as_chunks::<4>().0.iter().all(|px| px[3] == 255));
        assert!(frame.data.as_chunks::<4>().0.iter().any(|px| px[0] > 50));
        let start = Instant::now();
        let mut frames = 0;
        while start.elapsed() < Duration::from_millis(1000) {
            if let Some(f) = p.take_frame() {
                frames += 1;
                p.recycle(f.data);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!((10..=32).contains(&frames), "{frames} frames in 1 s");
        p.command("set pause yes");
        p.command("add volume -30");
        p.command("cycle mute");
        let st = wait_for(&p, |s| s.paused && s.muted);
        assert!(st.paused && st.muted, "{st:?}");
        assert_eq!(st.volume.round(), 50.0);
        assert!(st.duration > 1.5 && st.duration < 2.5, "{st:?}");
        p.command("seek 1.5 absolute");
        let st = wait_for(&p, |s| (s.pos - 1.5).abs() < 0.1);
        assert!((st.pos - 1.5).abs() < 0.1, "{st:?}");
        p.command("set pause no");
        let st = wait_for(&p, |s| s.eof);
        assert!(st.eof && !st.ended, "{st:?}");
        assert!(notified.load(std::sync::atomic::Ordering::SeqCst) > 0);
        p.join();
    }

    #[test]
    fn sixty_fps_video_is_shown_at_thirty() {
        if probe().is_err() {
            return;
        }
        let Some(path) = clip(
            "v60.mp4",
            &["testsrc2=size=320x180:rate=60:duration=3"],
            &["-c:v", "libx264", "-pix_fmt", "yuv420p"],
        ) else {
            return;
        };
        let p = Player::start(&path, null_audio(MediaKind::Animation), || {}).unwrap();
        p.want(Place {
            cols: 32,
            rows: 9,
            w: 320,
            h: 180,
        });
        let st = wait_for(&p, |s| s.fps > 50.0);
        assert!(st.fps > 50.0, "{st:?}");
        std::thread::sleep(Duration::from_millis(500));
        let start = Instant::now();
        let mut frames = 0;
        while start.elapsed() < Duration::from_millis(1500) {
            if let Some(f) = p.take_frame() {
                frames += 1;
                p.recycle(f.data);
            }
            std::thread::sleep(Duration::from_millis(3));
        }
        assert!((36..=47).contains(&frames), "{frames} frames in 1.5 s");
        p.join();
    }

    #[test]
    fn broken_files_end_with_an_error() {
        if probe().is_err() {
            return;
        }
        let path = std::env::temp_dir().join(format!("gramtty-bad-{}.mp4", std::process::id()));
        std::fs::write(&path, b"not a video at all").unwrap();
        let p =
            Player::start(&path.to_string_lossy(), null_audio(MediaKind::Video), || {}).unwrap();
        let st = wait_for(&p, |s| s.ended);
        assert!(st.ended, "{st:?}");
        assert!(
            st.error
                .as_deref()
                .is_some_and(|e| e.starts_with("cannot play")),
            "{st:?}"
        );
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn audio_plays_without_video_frames() {
        if probe().is_err() {
            return;
        }
        let Some(path) = clip("a.ogg", &["sine=f=440:duration=1"], &["-c:a", "libopus"]) else {
            return;
        };
        let p = Player::start(&path, null_audio(MediaKind::Voice), || {}).unwrap();
        p.want(Place {
            cols: 10,
            rows: 5,
            w: 100,
            h: 100,
        });
        let st = wait_for(&p, |s| s.eof);
        assert!(st.eof && st.error.is_none() && st.dims.is_none(), "{st:?}");
        assert!(p.take_frame().is_none());
        p.join();
    }
}
