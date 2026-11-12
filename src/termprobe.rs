use std::io::{self, IsTerminal, Write};
#[cfg(unix)]
use std::os::fd::AsFd;
#[cfg(windows)]
use std::os::windows::io::{AsHandle, AsRawHandle};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use crate::termcolors::{FALLBACK, Rgb, TermPalette, parse_spec};

pub const TIMEOUT: Duration = Duration::from_millis(1000);
pub const SETTLE_TIMEOUT: Duration = Duration::from_millis(300);
pub const LATE_WINDOW: Duration = Duration::from_secs(5);
const LATE_STRING_MAX: usize = 128;

static OWED: AtomicUsize = AtomicUsize::new(0);

#[cfg(unix)]
pub trait Input: AsFd {}
#[cfg(unix)]
impl<T: AsFd> Input for T {}
#[cfg(windows)]
pub trait Input: AsHandle {}
#[cfg(windows)]
impl<T: AsHandle> Input for T {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reply {
    Osc(String),
    Apc(String),
    CursorPosition { row: u16, col: u16 },
    DeviceAttributes(Vec<u16>),
    CellSize { width: u16, height: u16 },
    Status,
    Other(Vec<u8>),
}

#[derive(Default)]
pub struct Scanner {
    buf: Vec<u8>,
}

enum Scan {
    Done(Reply, usize),
    Incomplete,
}

impl Scanner {
    pub fn push(&mut self, bytes: &[u8]) -> Vec<Reply> {
        self.buf.extend_from_slice(bytes);
        let mut out = Vec::new();
        let mut i = 0;
        let mut text_start = 0;
        while i < self.buf.len() {
            if self.buf[i] != 0x1b {
                i += 1;
                continue;
            }
            match scan(&self.buf[i..]) {
                Scan::Incomplete => break,
                Scan::Done(reply, len) => {
                    if text_start < i {
                        out.push(Reply::Other(self.buf[text_start..i].to_vec()));
                    }
                    out.push(reply);
                    i += len;
                    text_start = i;
                }
            }
        }
        if text_start < i {
            out.push(Reply::Other(self.buf[text_start..i].to_vec()));
        }
        self.buf.drain(..i);
        out
    }

    #[cfg(test)]
    pub fn pending(&self) -> &[u8] {
        &self.buf
    }
}

fn scan(b: &[u8]) -> Scan {
    let Some(&kind) = b.get(1) else {
        return Scan::Incomplete;
    };
    match kind {
        b']' | b'_' | b'P' => {
            let mut j = 2;
            while j < b.len() {
                if b[j] == 0x07 && kind == b']' {
                    return Scan::Done(string_reply(kind, &b[2..j]), j + 1);
                }
                if b[j] == 0x1b {
                    match b.get(j + 1) {
                        None => return Scan::Incomplete,
                        Some(b'\\') => return Scan::Done(string_reply(kind, &b[2..j]), j + 2),
                        Some(_) => return Scan::Done(Reply::Other(b[..j].to_vec()), j),
                    }
                }
                j += 1;
            }
            Scan::Incomplete
        }
        b'[' => {
            let mut j = 2;
            while j < b.len() && !(0x40..=0x7e).contains(&b[j]) {
                if b[j] == 0x1b {
                    return Scan::Done(Reply::Other(b[..j].to_vec()), j);
                }
                j += 1;
            }
            if j >= b.len() {
                return Scan::Incomplete;
            }
            Scan::Done(csi_reply(&b[2..j], b[j], &b[..=j]), j + 1)
        }
        _ => Scan::Done(Reply::Other(b[..1].to_vec()), 1),
    }
}

fn string_reply(kind: u8, body: &[u8]) -> Reply {
    let text = String::from_utf8_lossy(body).into_owned();
    match kind {
        b']' => Reply::Osc(text),
        b'_' => Reply::Apc(text),
        _ => Reply::Other(body.to_vec()),
    }
}

fn csi_reply(params: &[u8], fin: u8, raw: &[u8]) -> Reply {
    let private = params.first() == Some(&b'?');
    let body = if private { &params[1..] } else { params };
    let nums: Option<Vec<u16>> = std::str::from_utf8(body).ok().and_then(|s| {
        if s.is_empty() {
            return Some(Vec::new());
        }
        s.split(';')
            .map(|p| {
                if p.is_empty() {
                    Some(0)
                } else {
                    p.parse().ok()
                }
            })
            .collect()
    });
    match (fin, private, nums.as_deref()) {
        (b'R', false, Some(&[row, col])) => Reply::CursorPosition { row, col },
        (b'c', true, Some(n)) => Reply::DeviceAttributes(n.to_vec()),
        (b't', false, Some(&[6, height, width])) => Reply::CellSize { width, height },
        (b'n', false, Some(&[0])) => Reply::Status,
        _ => Reply::Other(raw.to_vec()),
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ImageQuery {
    pub tmux: bool,
    pub graphics: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Probe {
    pub palette: bool,
    pub images: Option<ImageQuery>,
}

impl Probe {
    pub fn bytes(&self) -> String {
        let mut q = String::new();
        if self.palette {
            q.push_str("\x1b]10;?\x1b\\\x1b]11;?\x1b\\");
            for i in 0..16 {
                q.push_str(&format!("\x1b]4;{i};?\x1b\\"));
            }
        }
        if let Some(img) = self.images {
            let mut inner = String::new();
            if img.graphics {
                inner.push_str("\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\");
            }
            inner.push_str("\x1b[16t");
            if img.tmux {
                inner.push_str("\x1b[c");
                q.push_str("\x1bPtmux;");
                q.push_str(&inner.replace('\x1b', "\x1b\x1b"));
                q.push_str("\x1b\\");
            } else {
                q.push_str(&inner);
            }
        }
        q.push_str("\x1b[c");
        q
    }

    pub fn sentinels(&self) -> usize {
        1 + usize::from(self.images.is_some_and(|i| i.tmux))
    }

    pub fn string_replies(&self) -> usize {
        let palette = if self.palette { 18 } else { 0 };
        palette + usize::from(self.images.is_some_and(|i| i.graphics))
    }
}

#[derive(Debug, Default, PartialEq)]
pub struct Answers {
    pub fg: Option<Rgb>,
    pub bg: Option<Rgb>,
    pub ansi: [Option<Rgb>; 16],
    pub cell: Option<(u16, u16)>,
    pub kitty: bool,
    pub sixel: bool,
    pub cursor: Vec<(u16, u16)>,
    pub sentinels: usize,
    pub stray: Vec<u8>,
}

impl Answers {
    pub fn route(&mut self, reply: Reply) {
        match reply {
            Reply::Osc(body) => self.osc(&body),
            Reply::Apc(body) => {
                if body.starts_with("Gi=31;") {
                    self.kitty = body.ends_with(";OK");
                }
            }
            Reply::CursorPosition { row, col } => self.cursor.push((row, col)),
            Reply::DeviceAttributes(params) => {
                self.sentinels += 1;
                if params.iter().skip(1).any(|p| *p == 4) {
                    self.sixel = true;
                }
            }
            Reply::CellSize { width, height } => {
                if width > 0 && height > 0 {
                    self.cell = Some((width, height));
                }
            }
            Reply::Status => {}
            Reply::Other(bytes) => self.stray.extend_from_slice(&bytes),
        }
    }

    fn osc(&mut self, body: &str) {
        let mut it = body.splitn(2, ';');
        let (Some(code), Some(rest)) = (it.next(), it.next()) else {
            return;
        };
        match code {
            "10" => self.fg = parse_spec(rest),
            "11" => self.bg = parse_spec(rest),
            "4" => {
                let mut it = rest.splitn(2, ';');
                if let (Some(Ok(n)), Some(spec)) = (it.next().map(str::parse::<usize>), it.next())
                    && n < 16
                {
                    self.ansi[n] = parse_spec(spec);
                }
            }
            _ => {}
        }
    }

    pub fn string_replies(&self) -> usize {
        usize::from(self.fg.is_some())
            + usize::from(self.bg.is_some())
            + self.ansi.iter().filter(|c| c.is_some()).count()
    }

    pub fn palette(&self) -> Option<TermPalette> {
        let mut ansi = FALLBACK.ansi;
        for (slot, got) in ansi.iter_mut().zip(self.ansi) {
            if let Some(c) = got {
                *slot = c;
            }
        }
        Some(TermPalette {
            fg: self.fg?,
            bg: self.bg?,
            ansi,
        })
    }
}

#[derive(Debug, Default, PartialEq)]
pub struct Outcome {
    pub answers: Answers,
    pub complete: bool,
    pub owed: usize,
}

pub fn feed(answers: &mut Answers, scanner: &mut Scanner, bytes: &[u8]) {
    for r in scanner.push(bytes) {
        answers.route(r);
    }
}

#[cfg(unix)]
fn wait_readable(input: &impl AsFd, left: Duration) -> io::Result<bool> {
    use rustix::event::{PollFd, PollFlags, Timespec, poll};
    let ts = Timespec {
        tv_sec: left.as_secs() as _,
        tv_nsec: left.subsec_nanos() as _,
    };
    let mut fds = [PollFd::new(input, PollFlags::IN)];
    loop {
        match poll(&mut fds, Some(&ts)) {
            Ok(n) => return Ok(n > 0),
            Err(rustix::io::Errno::INTR) => continue,
            Err(e) => return Err(e.into()),
        }
    }
}

#[cfg(unix)]
fn flush_input(input: &impl Input) -> io::Result<()> {
    match rustix::termios::tcflush(input, rustix::termios::QueueSelector::IFlush) {
        Ok(()) | Err(rustix::io::Errno::NOTTY) => Ok(()),
        Err(e) => Err(e.into()),
    }
}

#[cfg(windows)]
fn flush_input(input: &impl Input) -> io::Result<()> {
    use windows_sys::Win32::System::Console::FlushConsoleInputBuffer;
    if unsafe { FlushConsoleInputBuffer(input.as_handle().as_raw_handle()) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(windows)]
struct VtInput {
    handle: windows_sys::Win32::Foundation::HANDLE,
    mode: u32,
}

#[cfg(windows)]
impl VtInput {
    fn enable(handle: windows_sys::Win32::Foundation::HANDLE) -> io::Result<Self> {
        use windows_sys::Win32::System::Console::{
            ENABLE_VIRTUAL_TERMINAL_INPUT, GetConsoleMode, SetConsoleMode,
        };
        let mut mode = 0;
        if unsafe { GetConsoleMode(handle, &mut mode) } == 0
            || unsafe { SetConsoleMode(handle, mode | ENABLE_VIRTUAL_TERMINAL_INPUT) } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { handle, mode })
    }
}

#[cfg(windows)]
impl Drop for VtInput {
    fn drop(&mut self) {
        unsafe { windows_sys::Win32::System::Console::SetConsoleMode(self.handle, self.mode) };
    }
}

#[cfg(windows)]
fn read_until(
    input: &impl Input,
    deadline: Instant,
    mut done: impl FnMut(&[u8]) -> bool,
) -> io::Result<bool> {
    use windows_sys::Win32::Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT};
    use windows_sys::Win32::System::Console::{INPUT_RECORD, KEY_EVENT, ReadConsoleInputW};
    use windows_sys::Win32::System::Threading::WaitForSingleObject;
    let handle = input.as_handle().as_raw_handle();
    let _vt = VtInput::enable(handle)?;
    let mut records = [INPUT_RECORD::default(); 128];
    let mut units = Vec::new();
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Ok(false);
        }
        let ms = u32::try_from(left.as_millis())
            .unwrap_or(u32::MAX - 1)
            .max(1);
        match unsafe { WaitForSingleObject(handle, ms) } {
            WAIT_OBJECT_0 => {}
            WAIT_TIMEOUT => return Ok(false),
            _ => return Err(io::Error::last_os_error()),
        }
        let mut n = 0;
        let len = records.len() as u32;
        if unsafe { ReadConsoleInputW(handle, records.as_mut_ptr(), len, &mut n) } == 0 {
            return Err(io::Error::last_os_error());
        }
        units.clear();
        for r in &records[..n as usize] {
            if u32::from(r.EventType) != KEY_EVENT {
                continue;
            }
            let key = unsafe { r.Event.KeyEvent };
            let unit = unsafe { key.uChar.UnicodeChar };
            if key.bKeyDown != 0 && unit != 0 {
                units.extend(std::iter::repeat_n(
                    unit,
                    usize::from(key.wRepeatCount.max(1)),
                ));
            }
        }
        if !units.is_empty() && done(String::from_utf16_lossy(&units).as_bytes()) {
            return Ok(true);
        }
    }
}

#[cfg(unix)]
fn read_until(
    input: &impl AsFd,
    deadline: Instant,
    mut done: impl FnMut(&[u8]) -> bool,
) -> io::Result<bool> {
    let mut chunk = [0u8; 1024];
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() || !wait_readable(input, left)? {
            return Ok(false);
        }
        match rustix::io::read(input, &mut chunk) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "terminal input closed",
                ));
            }
            Ok(n) => {
                if done(&chunk[..n]) {
                    return Ok(true);
                }
            }
            Err(rustix::io::Errno::INTR | rustix::io::Errno::AGAIN) => {}
            Err(e) => return Err(e.into()),
        }
    }
}

pub fn exchange(
    input: impl Input,
    out: &mut impl Write,
    probe: &Probe,
    timeout: Duration,
) -> io::Result<Outcome> {
    out.write_all(probe.bytes().as_bytes())?;
    out.flush()?;
    let deadline = Instant::now() + timeout;
    let want = probe.sentinels();
    let mut scanner = Scanner::default();
    let mut answers = Answers::default();
    let complete = read_until(&input, deadline, |bytes| {
        feed(&mut answers, &mut scanner, bytes);
        answers.sentinels >= want
    })?;
    Ok(Outcome {
        owed: want.saturating_sub(answers.sentinels),
        answers,
        complete,
    })
}

pub fn settle(input: impl Input, owed: usize, timeout: Duration) -> io::Result<usize> {
    if owed == 0 {
        return Ok(0);
    }
    let mut scanner = Scanner::default();
    let mut answers = Answers::default();
    read_until(&input, Instant::now() + timeout, |bytes| {
        feed(&mut answers, &mut scanner, bytes);
        answers.sentinels >= owed
    })?;
    flush_input(&input)?;
    Ok(owed.saturating_sub(answers.sentinels))
}

pub fn query_stdio(probe: &Probe, timeout: Duration) -> io::Result<Option<Outcome>> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    if !stdin.is_terminal() || !stdout.is_terminal() {
        return Ok(None);
    }
    #[cfg(windows)]
    crossterm::ansi_support::supports_ansi();
    let result = exchange(stdin_input(&stdin), &mut stdout.lock(), probe, timeout);
    let owed = match &result {
        Ok(o) => o.owed,
        Err(_) => probe.sentinels(),
    };
    OWED.fetch_add(owed, Ordering::SeqCst);
    result.map(Some)
}

pub fn settle_stdio() -> io::Result<()> {
    let stdin = io::stdin();
    if !stdin.is_terminal() {
        return Ok(());
    }
    let owed = OWED.swap(0, Ordering::SeqCst);
    let left = settle(stdin_input(&stdin), owed, SETTLE_TIMEOUT);
    if let Ok(n) = left {
        OWED.fetch_add(n, Ordering::SeqCst);
    } else {
        OWED.fetch_add(owed, Ordering::SeqCst);
    }
    left.map(|_| ())
}

#[cfg(unix)]
fn stdin_input(stdin: &io::Stdin) -> impl Input + '_ {
    stdin.as_fd()
}

#[cfg(windows)]
fn stdin_input(stdin: &io::Stdin) -> impl Input + '_ {
    stdin.as_handle()
}

pub fn owed() -> usize {
    OWED.load(Ordering::SeqCst)
}

#[derive(Debug, Default)]
pub struct LateReplies {
    until: Option<Instant>,
    strings: usize,
    inside: Option<usize>,
}

impl LateReplies {
    pub fn expect(&mut self, strings: usize, now: Instant) {
        if strings == 0 {
            return;
        }
        self.until = Some(now + LATE_WINDOW);
        self.strings = strings;
        self.inside = None;
    }

    #[cfg(test)]
    pub fn armed(&self) -> bool {
        self.until.is_some() || self.inside.is_some()
    }

    pub fn swallow(&mut self, event: &crossterm::event::Event, now: Instant) -> bool {
        use crossterm::event::{Event, KeyCode, KeyModifiers};
        let Event::Key(key) = event else {
            return false;
        };
        if let Some(n) = self.inside {
            let end = match key.code {
                KeyCode::Char('\\') => key.modifiers.contains(KeyModifiers::ALT),
                KeyCode::Char('g') => key.modifiers.contains(KeyModifiers::CONTROL),
                _ => false,
            };
            if end || n + 1 >= LATE_STRING_MAX {
                self.inside = None;
                self.strings = self.strings.saturating_sub(1);
                if self.strings == 0 {
                    self.until = None;
                }
            } else {
                self.inside = Some(n + 1);
            }
            return true;
        }
        match self.until {
            Some(t) if now < t => {}
            _ => {
                self.until = None;
                return false;
            }
        }
        let start = matches!(key.code, KeyCode::Char(']') | KeyCode::Char('_'))
            && key.modifiers.contains(KeyModifiers::ALT);
        if start {
            self.inside = Some(0);
        }
        start
    }
}

#[cfg(not(unix))]
pub fn font_size_from_winsize() -> Option<(u16, u16)> {
    None
}

#[cfg(unix)]
pub fn font_size_from_winsize() -> Option<(u16, u16)> {
    let ws = rustix::termios::tcgetwinsize(io::stdout()).ok()?;
    if ws.ws_xpixel == 0 || ws.ws_ypixel == 0 || ws.ws_col == 0 || ws.ws_row == 0 {
        return None;
    }
    Some((ws.ws_xpixel / ws.ws_col, ws.ws_ypixel / ws.ws_row))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route_all(chunks: &[&[u8]]) -> (Answers, Scanner) {
        let mut a = Answers::default();
        let mut s = Scanner::default();
        for c in chunks {
            feed(&mut a, &mut s, c);
        }
        (a, s)
    }

    const KITTY_REPLY: &[u8] = b"\x1b]10;rgb:e3e3/e2e2/d9d9\x1b\\\x1b]11;rgb:1212/1414/0e0e\x07\x1b]4;2;rgb:b5b5/cdcd/8f8f\x1b\\\x1b[48;205R\x1b]4;12;rgb:a0/c0/ff\x07\x1b_Gi=31;OK\x1b\\\x1b[6;20;10t\x1b[?62;4;22c";

    #[test]
    fn interleaved_osc_csi_r_and_device_attributes_route_to_owners() {
        let (a, s) = route_all(&[KITTY_REPLY]);
        assert_eq!(a.fg, Some((227, 226, 217)));
        assert_eq!(a.bg, Some((18, 20, 14)));
        assert_eq!(a.ansi[2], Some((181, 205, 143)));
        assert_eq!(a.ansi[12], Some((160, 192, 255)));
        assert_eq!(a.cursor, vec![(48, 205)]);
        assert!(a.kitty);
        assert!(a.sixel);
        assert_eq!(a.cell, Some((10, 20)));
        assert_eq!(a.sentinels, 1);
        assert!(a.stray.is_empty());
        assert!(s.pending().is_empty());
        let p = a.palette().unwrap();
        assert_eq!(p.ansi[2], (181, 205, 143));
        assert_eq!(p.ansi[0], FALLBACK.ansi[0]);
    }

    #[test]
    fn byte_by_byte_reads_give_the_same_result_as_one_read() {
        let chunks: Vec<&[u8]> = KITTY_REPLY.chunks(1).collect();
        let (one, _) = route_all(&[KITTY_REPLY]);
        let (split, s) = route_all(&chunks);
        assert_eq!(one, split);
        for cut in 1..KITTY_REPLY.len() {
            let (a, _) = route_all(&[&KITTY_REPLY[..cut], &KITTY_REPLY[cut..]]);
            assert_eq!(a, one, "split at {cut}");
        }
        assert!(s.pending().is_empty());
    }

    #[test]
    fn partial_sequence_waits_for_the_rest() {
        let mut a = Answers::default();
        let mut s = Scanner::default();
        feed(&mut a, &mut s, b"\x1b]10;rgb:ff/ff/ff");
        assert_eq!(a.fg, None);
        assert_eq!(s.pending(), b"\x1b]10;rgb:ff/ff/ff");
        feed(&mut a, &mut s, b"\x1b");
        assert_eq!(a.fg, None);
        feed(&mut a, &mut s, b"\\\x1b[12;");
        assert_eq!(a.fg, Some((255, 255, 255)));
        assert_eq!(s.pending(), b"\x1b[12;");
        feed(&mut a, &mut s, b"3R");
        assert_eq!(a.cursor, vec![(12, 3)]);
        assert!(s.pending().is_empty());
    }

    #[test]
    fn keystrokes_between_replies_are_kept_apart_as_stray_input() {
        let (a, _) = route_all(&[b"j\x1b[5;7Rk\x1b]11;#102030\x1b\\q", b"\x1b[?1;2c"]);
        assert_eq!(a.cursor, vec![(5, 7)]);
        assert_eq!(a.bg, Some((16, 32, 48)));
        assert_eq!(a.stray, b"jkq");
        assert_eq!(a.sentinels, 1);
        assert!(!a.sixel, "the class parameter is not a feature");
        assert_eq!(a.palette(), None);
    }

    #[test]
    fn kitty_style_device_attributes_with_trailing_separator_end_the_probe() {
        let (a, _) = route_all(&[b"\x1b[?62;c"]);
        assert_eq!(a.sentinels, 1);
        assert!(!a.sixel);
        assert!(a.stray.is_empty());
    }

    #[test]
    fn unrelated_sequences_do_not_count_as_answers() {
        let (a, _) =
            route_all(&[b"\x1b[A\x1b_Gi=31;ENOTSUPPORTED\x1b\\\x1b[0n\x1b[4;800;1000t\x1b[c"]);
        assert!(!a.kitty);
        assert_eq!(a.cell, None);
        assert_eq!(a.sentinels, 0);
        assert_eq!(a.stray, b"\x1b[A\x1b[4;800;1000t\x1b[c");
    }

    #[test]
    fn query_ends_with_one_sentinel_per_answering_terminal() {
        let palette = Probe {
            palette: true,
            images: None,
        };
        let q = palette.bytes();
        assert!(q.starts_with("\x1b]10;?\x1b\\\x1b]11;?\x1b\\"));
        assert_eq!(q.matches("\x1b]4;").count(), 16);
        assert!(q.ends_with("\x1b[c"));
        assert!(!q.contains("\x1b[6n"));
        assert_eq!(palette.sentinels(), 1);
        let direct = Probe {
            palette: true,
            images: Some(ImageQuery {
                tmux: false,
                graphics: true,
            }),
        };
        assert!(
            direct
                .bytes()
                .contains("\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\\x1b[16t")
        );
        assert_eq!(direct.bytes().matches("\x1b[c").count(), 1);
        let tmux = Probe {
            palette: false,
            images: Some(ImageQuery {
                tmux: true,
                graphics: false,
            }),
        };
        assert_eq!(tmux.bytes(), "\x1bPtmux;\x1b\x1b[16t\x1b\x1b[c\x1b\\\x1b[c");
        assert_eq!(tmux.sentinels(), 2);
    }

    #[cfg(unix)]
    #[test]
    fn exchange_writes_the_query_and_stops_at_the_sentinel() {
        let (reader, mut writer) = std::io::pipe().unwrap();
        writer
            .write_all(b"\x1b]10;rgb:dddd/dddd/dddd\x1b\\\x1b]11;rgb:0000/0000/0000\x1b\\\x1b[?62c")
            .unwrap();
        let probe = Probe {
            palette: true,
            images: None,
        };
        let mut out = Vec::new();
        let o = exchange(&reader, &mut out, &probe, Duration::from_secs(2)).unwrap();
        assert!(o.complete);
        let p = o.answers.palette().unwrap();
        assert_eq!(p.fg, (221, 221, 221));
        assert_eq!(p.bg, (0, 0, 0));
        assert_eq!(out, probe.bytes().as_bytes());
    }

    #[cfg(unix)]
    #[test]
    fn tmux_exchange_waits_for_both_device_attribute_replies() {
        let (reader, mut writer) = std::io::pipe().unwrap();
        writer.write_all(b"\x1b[?1;2c").unwrap();
        let probe = Probe {
            palette: false,
            images: Some(ImageQuery {
                tmux: true,
                graphics: true,
            }),
        };
        let t = Instant::now();
        let o = exchange(&reader, &mut Vec::new(), &probe, Duration::from_millis(120)).unwrap();
        assert!(!o.complete);
        assert!(t.elapsed() >= Duration::from_millis(100));
        let writer2 = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            writer
                .write_all(b"\x1b[?62;c\x1b_Gi=31;OK\x1b\\\x1b[6;18;9t\x1b[?62;4c")
                .unwrap();
            writer
        });
        let o = exchange(&reader, &mut Vec::new(), &probe, Duration::from_secs(2)).unwrap();
        writer2.join().unwrap();
        assert!(o.complete);
        assert!(o.answers.kitty);
        assert!(o.answers.sixel);
        assert_eq!(o.answers.cell, Some((9, 18)));
    }

    #[cfg(unix)]
    #[test]
    fn silent_terminal_times_out_and_returns_partial_answers() {
        let (reader, mut writer) = std::io::pipe().unwrap();
        writer.write_all(b"\x1b]11;rgb:10/20/30\x1b\\").unwrap();
        let probe = Probe {
            palette: true,
            images: None,
        };
        let t = Instant::now();
        let o = exchange(&reader, &mut Vec::new(), &probe, Duration::from_millis(80)).unwrap();
        let took = t.elapsed();
        assert!(!o.complete);
        assert_eq!(o.answers.bg, Some((16, 32, 48)));
        assert!(took >= Duration::from_millis(70), "{took:?}");
        assert!(took < Duration::from_millis(600), "{took:?}");
        drop(writer);
        let err = exchange(&reader, &mut Vec::new(), &probe, Duration::from_secs(5)).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
    }

    #[cfg(unix)]
    #[test]
    fn settle_waits_only_for_the_owed_late_reply() {
        let (reader, mut writer) = std::io::pipe().unwrap();
        let probe = Probe {
            palette: true,
            images: None,
        };
        let o = exchange(&reader, &mut Vec::new(), &probe, Duration::from_millis(20)).unwrap();
        assert!(!o.complete);
        assert_eq!(o.owed, 1);
        writer
            .write_all(b"\x1b]10;rgb:ff/ff/ff\x1b\\\x1b[?62c")
            .unwrap();
        assert_eq!(settle(&reader, o.owed, Duration::from_secs(2)).unwrap(), 0);
        assert_eq!(settle(&reader, 0, Duration::from_secs(2)).unwrap(), 0);
        let t = Instant::now();
        assert_eq!(settle(&reader, 1, Duration::from_millis(50)).unwrap(), 1);
        assert!(t.elapsed() >= Duration::from_millis(40));
        writer.write_all(b"k").unwrap();
        let mut rest = [0u8; 4];
        assert_eq!(rustix::io::read(&reader, &mut rest).unwrap(), 1);
    }

    fn crossterm_events(bytes: &[u8]) -> Vec<crossterm::event::Event> {
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
        let mut out = Vec::new();
        let mut i = 0;
        while i < bytes.len() {
            let (b, mods) = if bytes[i] == 0x1b && i + 1 < bytes.len() {
                i += 1;
                (bytes[i], KeyModifiers::ALT)
            } else {
                (bytes[i], KeyModifiers::NONE)
            };
            let key = match b {
                0x01..=0x1a => KeyEvent::new(
                    KeyCode::Char((b - 1 + b'a') as char),
                    mods | KeyModifiers::CONTROL,
                ),
                b => KeyEvent::new(KeyCode::Char(b as char), mods),
            };
            out.push(Event::Key(key));
            i += 1;
        }
        out
    }

    fn passed(late: &mut LateReplies, bytes: &[u8], now: Instant) -> String {
        crossterm_events(bytes)
            .iter()
            .filter(|e| !late.swallow(e, now))
            .map(|e| match e {
                crossterm::event::Event::Key(k) => match k.code {
                    crossterm::event::KeyCode::Char(c) => c,
                    _ => '?',
                },
                _ => '?',
            })
            .collect()
    }

    #[cfg(unix)]
    #[test]
    fn late_palette_replies_after_a_timeout_never_become_keystrokes() {
        let (reader, mut writer) = std::io::pipe().unwrap();
        let probe = Probe {
            palette: true,
            images: None,
        };
        writer
            .write_all(b"\x1b]10;rgb:e3e3/e3e3/d9d9\x1b\\")
            .unwrap();
        let o = exchange(&reader, &mut Vec::new(), &probe, Duration::from_millis(20)).unwrap();
        assert!(!o.complete);
        let expected = probe.string_replies() - o.answers.string_replies();
        assert_eq!(expected, 17);
        let now = Instant::now();
        let mut late = LateReplies::default();
        late.expect(expected, now);
        assert!(late.armed());
        let mut tail = b"\x1b]11;rgb:1212/1414/0e0e\x07".to_vec();
        for i in 0..16 {
            tail.extend_from_slice(format!("\x1b]4;{i};rgb:b5b5/cdcd/8f8f\x1b\\").as_bytes());
        }
        let (head, rest) = tail.split_at(13);
        assert_eq!(passed(&mut late, head, now), "");
        assert_eq!(passed(&mut late, rest, now + Duration::from_secs(1)), "");
        assert!(!late.armed());
        assert_eq!(passed(&mut late, b"rgb/", now), "rgb/");
    }

    #[test]
    fn keys_around_late_replies_pass_and_the_window_expires() {
        let now = Instant::now();
        let mut late = LateReplies::default();
        late.expect(18, now);
        assert_eq!(
            passed(&mut late, b"jk\x1b]10;rgb:ff/ff/ff\x1b\\q", now),
            "jkq"
        );
        assert!(late.armed());
        let after = now + LATE_WINDOW;
        assert_eq!(passed(&mut late, b"\x1b]x", after), "]x");
        assert!(!late.armed());
        let mut idle = LateReplies::default();
        idle.expect(0, now);
        assert!(!idle.armed());
        assert_eq!(passed(&mut idle, b"\x1b]11;", now), "]11;");
    }

    #[test]
    fn unterminated_late_reply_stops_swallowing_after_a_bound() {
        let now = Instant::now();
        let mut late = LateReplies::default();
        late.expect(1, now);
        let mut bytes = b"\x1b]".to_vec();
        bytes.extend(std::iter::repeat_n(b'a', LATE_STRING_MAX + 5));
        let kept = passed(&mut late, &bytes, now);
        assert_eq!(kept.len(), 5);
        assert!(!late.armed());
    }
}
