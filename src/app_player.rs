use super::{App, FileJob, attachment_name};
use crate::audio::{self, Track};
use crate::model::{Attachment, MediaKind, Msg};
use crate::td::Event;

fn spoken(kind: MediaKind) -> bool {
    matches!(kind, MediaKind::Voice | MediaKind::VideoNote)
}

fn queued_kind(kind: MediaKind) -> bool {
    matches!(kind, MediaKind::Voice | MediaKind::Audio)
}

fn same_family(first: MediaKind, other: MediaKind) -> bool {
    match first {
        MediaKind::Audio => other == MediaKind::Audio,
        _ => spoken(other),
    }
}

impl App {
    pub(super) fn play_in_background(&mut self, msg: &Msg) -> bool {
        let Some(att) = msg.attachments.first() else {
            return false;
        };
        if !queued_kind(att.kind) {
            return false;
        }
        if let Err(e) = self.bg.available() {
            self.play_attachment(msg.chat_id, msg.id, 0, att.clone());
            if !self.bg.unavailable_noted {
                self.bg.unavailable_noted = true;
                self.set_status(format!("{e}; playing in mpv instead"), true);
            }
            return true;
        }
        let queue = self.build_queue(msg.chat_id, msg.id, att.kind);
        self.drop_background_jobs();
        self.bg.set_queue(queue, 0);
        self.start_track();
        true
    }

    fn build_queue(&self, chat_id: i64, msg_id: i64, first: MediaKind) -> Vec<Track> {
        let Some(h) = self.histories.get(&chat_id) else {
            return Vec::new();
        };
        let from = h.index_of(msg_id).unwrap_or(0);
        h.msgs()[from..]
            .iter()
            .filter(|m| !m.deleted)
            .filter_map(|m| {
                let att = m.attachments.first()?;
                same_family(first, att.kind).then(|| self.track_for(m, att))
            })
            .collect()
    }

    fn track_for(&self, msg: &Msg, att: &Attachment) -> Track {
        let title = if att.kind == MediaKind::Audio {
            msg.media
                .as_deref()
                .and_then(|m| m.strip_prefix("♪ "))
                .filter(|t| !t.is_empty())
                .map_or_else(|| att.name.clone(), str::to_owned)
        } else {
            self.sender_name(msg.sender)
        };
        Track {
            chat_id: msg.chat_id,
            msg_id: msg.id,
            idx: 0,
            kind: att.kind,
            title,
            duration: att.duration,
        }
    }

    fn drop_background_jobs(&mut self) {
        self.file_jobs
            .retain(|(_, j)| !matches!(j, FileJob::Background(_)));
    }

    fn start_track(&mut self) {
        let Some(track) = self.bg.track().cloned() else {
            return;
        };
        let Some(att) = self
            .attachment(track.chat_id, track.msg_id, track.idx)
            .cloned()
        else {
            self.set_status("this track is no longer available", true);
            self.advance_background();
            return;
        };
        let name = attachment_name(&att, track.msg_id);
        self.queue_file_job(
            &att.file,
            &name,
            FileJob::Background((track.msg_id, track.idx)),
        );
    }

    pub(super) fn background_loaded(&mut self, path: &str, target: (i64, usize)) {
        let Some(track) = self.bg.track().cloned() else {
            return;
        };
        if (track.msg_id, track.idx) != target {
            return;
        }
        self.inline.stop();
        self.stop_playback();
        let speed = if spoken(track.kind) {
            self.settings.media.voice_speed()
        } else {
            1.0
        };
        let tx = self.td.sender();
        let notify = move || {
            let _ = tx.send(Event::Audio);
        };
        match self.bg.load(path, speed, self.inline.sound(), notify) {
            Ok(()) => {
                self.status = None;
                self.mark_listened(&track);
            }
            Err(e) => {
                self.set_status(format!("cannot play {}: {e}", track.title), true);
                self.bg.stop();
            }
        }
    }

    fn mark_listened(&mut self, track: &Track) {
        if !spoken(track.kind) || !self.settings.auto_read() {
            return;
        }
        let incoming = self
            .histories
            .get(&track.chat_id)
            .and_then(|h| h.get(track.msg_id))
            .is_some_and(|m| !m.outgoing);
        if incoming {
            self.td.open_content(track.chat_id, track.msg_id);
        }
    }

    pub(super) fn audio_tick(&mut self) {
        if !self.bg.active() || !self.bg.loaded() {
            return;
        }
        if let Some(e) = self.bg.failure() {
            let title = self.bg.track().map(|t| t.title.clone()).unwrap_or_default();
            self.set_status(format!("cannot play {title}: {e}"), true);
            self.bg.stop();
            return;
        }
        if self.bg.finished() {
            self.advance_background();
        }
    }

    fn advance_background(&mut self) {
        self.drop_background_jobs();
        if self.bg.select(self.bg.index() + 1) {
            self.start_track();
        } else {
            self.bg.stop();
        }
    }

    pub(super) fn pause_background(&mut self) {
        if self.bg.active() && !self.bg.paused() {
            self.bg.set_paused(true);
        }
    }

    pub(super) fn player_pause(&mut self) {
        if self.bg.loaded() {
            self.bg.toggle_pause();
        }
    }

    pub(super) fn player_stop(&mut self) {
        if self.bg.active() {
            self.drop_background_jobs();
            self.bg.stop();
            self.set_status("player stopped", false);
        }
    }

    pub(super) fn player_next(&mut self) {
        if self.bg.active() {
            self.advance_background();
        }
    }

    pub(super) fn player_previous(&mut self) {
        if !self.bg.active() {
            return;
        }
        let played = self.bg.status().map_or(0.0, |s| s.pos);
        if played > 3.0 || self.bg.index() == 0 {
            self.bg.seek_to(0.0);
            return;
        }
        self.drop_background_jobs();
        if self.bg.select(self.bg.index() - 1) {
            self.start_track();
        }
    }

    pub(super) fn player_seek(&mut self, secs: f64) {
        self.bg.seek(secs);
    }

    pub(super) fn player_speed(&mut self) {
        let Some(kind) = self.bg.track().map(|t| t.kind) else {
            return;
        };
        let speed = audio::next_speed(self.bg.speed());
        self.bg.set_speed(speed);
        if spoken(kind) {
            self.settings.media.remember_voice_speed(speed);
            self.save_settings();
        }
        self.set_status(format!("speed {}", audio::speed_text(speed)), false);
    }
}

#[cfg(target_os = "linux")]
impl App {
    pub(super) fn mpris_sync(&mut self) {
        if !self.settings.media.mpris {
            self.mpris = None;
            return;
        }
        if !self.bg.live() || (!self.bg.active() && self.mpris.is_none()) {
            return;
        }
        if self.mpris.is_none() {
            if self.mpris_failed {
                return;
            }
            let (tx, failed) = (self.td.sender(), self.td.sender());
            self.mpris = Some(crate::mpris::spawn(
                None,
                move |c| {
                    let _ = tx.send(Event::Mpris(c));
                },
                move |e| {
                    let _ = failed.send(Event::MprisFailed(e));
                },
            ));
        }
        let state = self.mpris_state();
        if let Some(h) = &mut self.mpris {
            h.push(state);
        }
    }

    fn mpris_state(&mut self) -> crate::mpris::State {
        use crate::mpris::{self, Playback, State};
        let Some(t) = self.bg.track().cloned() else {
            return State::default();
        };
        let st = self.bg.status().unwrap_or_default();
        let hint = self.bg.take_seek_hint();
        let (title, artist) = if t.kind == MediaKind::Audio {
            match t.title.split_once(" – ") {
                Some((a, s)) => (s.to_owned(), vec![a.to_owned()]),
                None => (t.title.clone(), Vec::new()),
            }
        } else {
            ("Voice message".to_owned(), vec![t.title.clone()])
        };
        let length = if st.duration > 0.0 {
            st.duration
        } else {
            f64::from(t.duration)
        };
        let (volume, muted) = self.inline.sound();
        State {
            playback: if self.bg.paused() {
                Playback::Paused
            } else {
                Playback::Playing
            },
            track: Some(mpris::Track {
                path: mpris::track_path(t.chat_id, t.msg_id),
                title,
                artist,
                length_us: mpris::micros(length),
            }),
            rate: self.bg.speed(),
            volume: if muted { 0.0 } else { volume / 100.0 },
            position_us: mpris::micros(hint.unwrap_or(st.pos)),
            can_next: self.bg.has_next(),
            can_previous: true,
            seeks: self.bg.seeks(),
        }
    }

    pub(super) fn on_mpris(&mut self, cmd: crate::mpris::Command) {
        use crate::mpris::{Command, seconds};
        match cmd {
            Command::PlayPause => self.player_pause(),
            Command::Play if self.bg.paused() => self.player_pause(),
            Command::Play => {}
            Command::Pause => self.pause_background(),
            Command::Stop => self.player_stop(),
            Command::Next => self.player_next(),
            Command::Previous => self.player_previous(),
            Command::Seek(us) => self.player_seek(seconds(us)),
            Command::SetPosition(us) => self.bg.seek_to(seconds(us)),
        }
    }
}
