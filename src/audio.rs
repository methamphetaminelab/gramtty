use crate::format;
use crate::model::MediaKind;
use crate::video::{Options, Player, Status};

pub const SEEK_STEP: f64 = 5.0;
const SPEEDS: [f64; 4] = [1.0, 1.25, 1.5, 2.0];

pub fn next_speed(current: f64) -> f64 {
    SPEEDS
        .iter()
        .copied()
        .find(|s| *s > current + 1e-9)
        .unwrap_or(SPEEDS[0])
}

pub fn speed_text(speed: f64) -> String {
    format!(
        "{}×",
        format!("{speed:.2}")
            .trim_end_matches('0')
            .trim_end_matches('.')
    )
}

#[derive(Clone, Debug, PartialEq)]
pub struct Track {
    pub chat_id: i64,
    pub msg_id: i64,
    pub idx: usize,
    pub kind: MediaKind,
    pub title: String,
    pub duration: i32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BarState {
    pub paused: bool,
    pub title: String,
    pub pos: f64,
    pub duration: f64,
    pub speed: f64,
    pub index: usize,
    pub total: usize,
}

pub fn bar_text(state: &BarState, width: usize) -> String {
    let icon = if state.paused { "⏸" } else { "▶" };
    let time = format!(
        "{} / {}",
        format::duration(state.pos as i32),
        format::duration(state.duration as i32)
    );
    let mut suffix = Vec::new();
    if (state.speed - 1.0).abs() > 1e-9 {
        suffix.push(speed_text(state.speed));
    }
    if state.total > 1 {
        suffix.push(format!("{}/{}", state.index + 1, state.total));
    }
    let suffix = suffix.join(" ");
    let suffix_w = if suffix.is_empty() {
        0
    } else {
        1 + format::cells(&suffix)
    };
    let fixed = 2 + format::cells(&time) + suffix_w;
    if width < fixed + 4 {
        let short = format!("{icon} {time}");
        return format::truncate(&short, width);
    }
    let avail = width - fixed;
    let title_w = format::cells(&state.title);
    let mut out = String::from(icon);
    if avail < 30 {
        let shown = format::truncate(&state.title, avail - 1);
        out.push(' ');
        out.push_str(&shown);
        out.push(' ');
        out.push_str(&time);
        if suffix_w > 0 {
            out.push(' ');
            out.push_str(&suffix);
        }
        return out;
    }
    let title = format::truncate(&state.title, title_w.min(avail / 3));
    let bar_w = avail - format::cells(&title) - 2;
    let filled = if state.duration > 0.0 {
        ((state.pos / state.duration).clamp(0.0, 1.0) * bar_w as f64).round() as usize
    } else {
        0
    };
    out.push(' ');
    out.push_str(&title);
    out.push(' ');
    out.push_str(&time);
    out.push(' ');
    out.push_str(&"━".repeat(filled));
    out.push_str(&"─".repeat(bar_w - filled));
    if suffix_w > 0 {
        out.push(' ');
        out.push_str(&suffix);
    }
    out
}

#[cfg(test)]
#[derive(Default)]
pub struct Fake {
    pub log: Vec<String>,
    pub status: Status,
}

enum Engine {
    Live(Option<Player>),
    #[cfg(test)]
    Fake(Fake),
}

pub struct Background {
    engine: Engine,
    pub audio_out: Option<String>,
    pub unavailable_noted: bool,
    queue: Vec<Track>,
    pos: usize,
    speed: f64,
    paused: bool,
    loaded: bool,
    waiting: bool,
    seeks: u64,
    seek_hint: Option<f64>,
}

impl Background {
    pub fn new() -> Self {
        Self::with(Engine::Live(None))
    }

    #[cfg(test)]
    pub fn fake() -> Self {
        Self::with(Engine::Fake(Fake::default()))
    }

    fn with(engine: Engine) -> Self {
        Self {
            engine,
            audio_out: None,
            unavailable_noted: false,
            queue: Vec::new(),
            pos: 0,
            speed: 1.0,
            paused: false,
            loaded: false,
            waiting: false,
            seeks: 0,
            seek_hint: None,
        }
    }

    pub fn live(&self) -> bool {
        matches!(self.engine, Engine::Live(_))
    }

    pub fn seeks(&self) -> u64 {
        self.seeks
    }

    pub fn take_seek_hint(&mut self) -> Option<f64> {
        self.seek_hint.take()
    }

    pub fn available(&self) -> Result<(), String> {
        match self.engine {
            Engine::Live(_) => crate::video::probe(),
            #[cfg(test)]
            Engine::Fake(_) => Ok(()),
        }
    }

    pub fn active(&self) -> bool {
        !self.queue.is_empty()
    }

    #[cfg(test)]
    pub fn queue(&self) -> &[Track] {
        &self.queue
    }

    pub fn index(&self) -> usize {
        self.pos
    }

    pub fn track(&self) -> Option<&Track> {
        self.queue.get(self.pos)
    }

    #[cfg(test)]
    pub fn chat_id(&self) -> Option<i64> {
        self.track().map(|t| t.chat_id)
    }

    pub fn speed(&self) -> f64 {
        self.speed
    }

    pub fn paused(&self) -> bool {
        self.paused
    }

    pub fn loaded(&self) -> bool {
        self.loaded
    }

    #[cfg(test)]
    pub fn waiting(&self) -> bool {
        self.waiting
    }

    pub fn set_queue(&mut self, queue: Vec<Track>, pos: usize) {
        self.release();
        self.pos = pos.min(queue.len().saturating_sub(1));
        self.queue = queue;
        self.waiting = true;
    }

    pub fn select(&mut self, pos: usize) -> bool {
        if pos >= self.queue.len() {
            return false;
        }
        self.release();
        self.pos = pos;
        self.waiting = true;
        true
    }

    pub fn has_next(&self) -> bool {
        self.pos + 1 < self.queue.len()
    }

    pub fn stop(&mut self) {
        self.release();
        self.queue.clear();
        self.pos = 0;
        self.waiting = false;
    }

    fn release(&mut self) {
        match &mut self.engine {
            Engine::Live(p) => drop(p.take()),
            #[cfg(test)]
            Engine::Fake(f) => {
                f.log.push("release".to_owned());
                f.status = Status::default();
            }
        }
        self.loaded = false;
        self.paused = false;
    }

    pub fn load(
        &mut self,
        path: &str,
        speed: f64,
        sound: (f64, bool),
        notify: impl Fn() + Send + 'static,
    ) -> Result<(), String> {
        self.speed = speed;
        self.paused = false;
        self.waiting = false;
        match &mut self.engine {
            Engine::Live(slot) => {
                let kind = self
                    .queue
                    .get(self.pos)
                    .map_or(MediaKind::Audio, |t| t.kind);
                let kind = if kind == MediaKind::Voice {
                    MediaKind::Voice
                } else {
                    MediaKind::Audio
                };
                let mut opts = Options::for_kind(kind, sound.0, sound.1);
                opts.audio_out.clone_from(&self.audio_out);
                let player = Player::start(path, opts, notify)?;
                if (speed - 1.0).abs() > 1e-9 {
                    player.command(format!("set speed {speed}"));
                }
                *slot = Some(player);
            }
            #[cfg(test)]
            Engine::Fake(f) => {
                f.log.push(format!("load {path} speed {speed}"));
                f.status = Status {
                    loaded: true,
                    duration: self
                        .queue
                        .get(self.pos)
                        .map_or(0.0, |t| f64::from(t.duration)),
                    volume: sound.0,
                    muted: sound.1,
                    ..Status::default()
                };
            }
        }
        self.loaded = true;
        Ok(())
    }

    fn command(&mut self, cmd: String) {
        match &mut self.engine {
            Engine::Live(Some(p)) => p.command(cmd),
            Engine::Live(None) => {}
            #[cfg(test)]
            Engine::Fake(f) => f.log.push(cmd),
        }
    }

    pub fn set_paused(&mut self, paused: bool) {
        if !self.loaded {
            return;
        }
        self.paused = paused;
        self.command(format!("set pause {}", if paused { "yes" } else { "no" }));
    }

    pub fn toggle_pause(&mut self) {
        self.set_paused(!self.paused);
    }

    pub fn seek(&mut self, secs: f64) {
        if self.loaded {
            let now = self.status().map_or(0.0, |s| s.pos);
            self.seek_hint = Some((now + secs).max(0.0));
            self.seeks += 1;
            self.command(format!("seek {secs} relative"));
        }
    }

    pub fn seek_to(&mut self, secs: f64) {
        if self.loaded {
            self.seek_hint = Some(secs.max(0.0));
            self.seeks += 1;
            self.command(format!("seek {secs} absolute"));
        }
    }

    pub fn set_speed(&mut self, speed: f64) {
        self.speed = speed;
        if self.loaded {
            self.command(format!("set speed {speed}"));
        }
    }

    pub fn status(&self) -> Option<Status> {
        if !self.loaded {
            return None;
        }
        match &self.engine {
            Engine::Live(p) => p.as_ref().map(Player::status),
            #[cfg(test)]
            Engine::Fake(f) => Some(f.status.clone()),
        }
    }

    pub fn finished(&self) -> bool {
        self.status().is_some_and(|s| s.ended || s.eof)
    }

    pub fn failure(&self) -> Option<String> {
        self.status().and_then(|s| s.error)
    }

    pub fn bar(&self) -> Option<BarState> {
        let track = self.track()?;
        let st = self.status().unwrap_or_default();
        Some(BarState {
            paused: self.paused,
            title: track.title.clone(),
            pos: st.pos,
            duration: if st.duration > 0.0 {
                st.duration
            } else {
                f64::from(track.duration)
            },
            speed: self.speed,
            index: self.pos,
            total: self.queue.len(),
        })
    }

    #[cfg(test)]
    pub fn fake_mut(&mut self) -> Option<&mut Fake> {
        match &mut self.engine {
            #[cfg(test)]
            Engine::Fake(f) => Some(f),
            Engine::Live(_) => None,
        }
    }

    #[cfg(test)]
    pub fn fake_log(&self) -> Vec<String> {
        match &self.engine {
            #[cfg(test)]
            Engine::Fake(f) => f.log.clone(),
            Engine::Live(_) => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn state() -> BarState {
        BarState {
            paused: false,
            title: "Alice Wonderland".to_owned(),
            pos: 42.0,
            duration: 190.0,
            speed: 1.0,
            index: 1,
            total: 7,
        }
    }

    #[test]
    fn speeds_cycle_through_the_presets() {
        assert_eq!(next_speed(1.0), 1.25);
        assert_eq!(next_speed(1.25), 1.5);
        assert_eq!(next_speed(1.5), 2.0);
        assert_eq!(next_speed(2.0), 1.0);
        assert_eq!(next_speed(1.1), 1.25);
        assert_eq!(next_speed(0.5), 1.0);
        assert_eq!(speed_text(1.5), "1.5×");
        assert_eq!(speed_text(2.0), "2×");
        assert_eq!(speed_text(1.25), "1.25×");
    }

    #[test]
    fn bar_shows_state_time_title_speed_and_position() {
        let line = bar_text(&state(), 80);
        assert_eq!(format::cells(&line), 80, "{line}");
        assert!(
            line.starts_with("▶ Alice Wonderland 0:42 / 3:10 "),
            "{line}"
        );
        assert!(line.ends_with(" 2/7"), "{line}");
        assert!(!line.contains('×'));
        let mut s = state();
        s.paused = true;
        s.speed = 1.5;
        s.total = 1;
        let line = bar_text(&s, 80);
        assert!(line.starts_with('⏸') && line.ends_with(" 1.5×"), "{line}");
        assert!(line.contains('━') && line.contains('─'));
    }

    #[test]
    fn bar_fits_every_width_without_panicking() {
        let mut s = state();
        s.speed = 2.0;
        s.title = "Очень длинное название трека 日本語のタイトル".to_owned();
        for width in [0, 1, 2, 5, 10, 13, 20, 30, 40, 80, 200] {
            let line = bar_text(&s, width);
            assert!(format::cells(&line) <= width, "{width}: {line}");
        }
        let narrow = bar_text(&s, 40);
        assert!(
            narrow.starts_with('▶') && narrow.contains("0:42 / 3:10"),
            "{narrow}"
        );
        assert!(narrow.contains('…'), "{narrow}");
        assert_eq!(format::cells(&bar_text(&s, 200)), 200);
    }

    #[test]
    fn bar_handles_unknown_duration() {
        let mut s = state();
        s.duration = 0.0;
        let line = bar_text(&s, 80);
        assert!(line.contains("0:42 / 0:00") && line.contains('─'), "{line}");
    }

    fn track(msg_id: i64) -> Track {
        Track {
            chat_id: 1,
            msg_id,
            idx: 0,
            kind: MediaKind::Voice,
            title: "t".to_owned(),
            duration: 10,
        }
    }

    #[test]
    fn fake_background_records_commands_and_tracks_state() {
        let mut bg = Background::fake();
        assert!(!bg.active() && bg.status().is_none());
        bg.set_queue(vec![track(1), track(2)], 0);
        assert!(bg.active() && bg.waiting());
        bg.load("/a.ogg", 1.5, (80.0, false), || {}).unwrap();
        assert!(!bg.waiting() && bg.loaded());
        bg.toggle_pause();
        assert!(bg.paused());
        bg.seek(5.0);
        bg.set_speed(2.0);
        assert_eq!(
            bg.fake_log(),
            [
                "release",
                "load /a.ogg speed 1.5",
                "set pause yes",
                "seek 5 relative",
                "set speed 2"
            ]
        );
        bg.fake_mut().unwrap().status.eof = true;
        assert!(bg.finished());
        bg.stop();
        assert!(!bg.active() && bg.status().is_none());
    }

    fn live_clip() -> Option<String> {
        if crate::video::probe().is_err() {
            return None;
        }
        if std::process::Command::new("ffmpeg")
            .arg("-version")
            .output()
            .is_err()
        {
            return None;
        }
        let dir = std::env::temp_dir().join(format!("gramtty-audio-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("a.ogg");
        let status = std::process::Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "sine=f=440:duration=2",
                "-c:a",
                "libopus",
            ])
            .arg(&out)
            .status()
            .unwrap();
        assert!(status.success());
        Some(out.to_string_lossy().into_owned())
    }

    fn wait_status(bg: &Background, ok: impl Fn(&Status) -> bool) -> Status {
        let until = Instant::now() + Duration::from_secs(10);
        loop {
            let s = bg.status().unwrap_or_default();
            if ok(&s) || Instant::now() > until {
                return s;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn plays_pauses_changes_speed_and_ends_without_video_frames() {
        let Some(path) = live_clip() else {
            return;
        };
        let mut bg = Background::new();
        bg.audio_out = Some("null".to_owned());
        bg.set_queue(vec![track(1)], 0);
        bg.load(&path, 1.0, (80.0, false), || {}).unwrap();
        let st = wait_status(&bg, |s| s.loaded && s.pos > 0.2);
        assert!(
            st.pos > 0.2 && st.error.is_none() && st.dims.is_none(),
            "{st:?}"
        );
        bg.set_paused(true);
        let st = wait_status(&bg, |s| s.paused);
        assert!(st.paused, "{st:?}");
        let held = st.pos;
        std::thread::sleep(Duration::from_millis(300));
        let after = bg.status().unwrap();
        assert!((after.pos - held).abs() < 0.2, "{held} {after:?}");
        bg.set_speed(2.0);
        bg.set_paused(false);
        let st = wait_status(&bg, |s| !s.paused);
        assert!(!st.paused);
        let st = wait_status(&bg, |s| s.eof);
        assert!(st.eof && st.error.is_none(), "{st:?}");
        assert!(bg.finished());
        bg.stop();
        assert!(bg.status().is_none());
    }
}
