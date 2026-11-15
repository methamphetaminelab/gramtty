use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{ObjectPath, OwnedValue, Value};
use zbus::{connection, interface};

pub const BUS_NAME: &str = "org.mpris.MediaPlayer2.gramtty";
pub const OBJECT_PATH: &str = "/org/mpris/MediaPlayer2";
const NO_TRACK: &str = "/org/mpris/MediaPlayer2/TrackList/NoTrack";

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    PlayPause,
    Play,
    Pause,
    Stop,
    Next,
    Previous,
    Seek(i64),
    SetPosition(i64),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Playback {
    #[default]
    Stopped,
    Playing,
    Paused,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Track {
    pub path: String,
    pub title: String,
    pub artist: Vec<String>,
    pub length_us: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct State {
    pub playback: Playback,
    pub track: Option<Track>,
    pub rate: f64,
    pub volume: f64,
    pub position_us: i64,
    pub can_next: bool,
    pub can_previous: bool,
    pub seeks: u64,
}

impl Default for State {
    fn default() -> Self {
        Self {
            playback: Playback::Stopped,
            track: None,
            rate: 1.0,
            volume: 1.0,
            position_us: 0,
            can_next: false,
            can_previous: false,
            seeks: 0,
        }
    }
}

pub fn status_str(playback: Playback) -> &'static str {
    match playback {
        Playback::Stopped => "Stopped",
        Playback::Playing => "Playing",
        Playback::Paused => "Paused",
    }
}

pub fn micros(secs: f64) -> i64 {
    if secs.is_finite() {
        (secs * 1_000_000.0).round() as i64
    } else {
        0
    }
}

pub fn seconds(micros: i64) -> f64 {
    micros as f64 / 1_000_000.0
}

pub fn track_path(chat_id: i64, msg_id: i64) -> String {
    format!(
        "/org/gramtty/track/c{}_m{}",
        chat_id.unsigned_abs(),
        msg_id.unsigned_abs()
    )
}

pub fn metadata(track: Option<&Track>) -> HashMap<String, OwnedValue> {
    let mut map = HashMap::new();
    let path = track.map_or(NO_TRACK, |t| t.path.as_str());
    let path = ObjectPath::try_from(path.to_owned())
        .unwrap_or_else(|_| ObjectPath::from_static_str_unchecked(NO_TRACK));
    let mut put = |key: &str, value: Value<'_>| {
        if let Ok(v) = OwnedValue::try_from(value) {
            map.insert(key.to_owned(), v);
        }
    };
    put("mpris:trackid", Value::ObjectPath(path));
    if let Some(t) = track {
        put("mpris:length", Value::I64(t.length_us));
        put("xesam:title", Value::from(t.title.clone()));
        put("xesam:artist", Value::from(t.artist.clone()));
    }
    map
}

pub fn position_now(state: &State, elapsed: Duration) -> i64 {
    let mut pos = state.position_us;
    if state.playback == Playback::Playing {
        pos += micros(elapsed.as_secs_f64() * state.rate);
    }
    let length = state.track.as_ref().map_or(0, |t| t.length_us);
    if length > 0 { pos.min(length) } else { pos }.max(0)
}

struct Shared {
    state: State,
    at: Instant,
}

type Callback = Arc<dyn Fn(Command) + Send + Sync>;

fn lock(shared: &Mutex<Shared>) -> MutexGuard<'_, Shared> {
    shared.lock().unwrap_or_else(PoisonError::into_inner)
}

struct Root;

#[interface(name = "org.mpris.MediaPlayer2")]
impl Root {
    fn raise(&self) {}

    fn quit(&self) {}

    #[zbus(property)]
    fn can_quit(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn can_raise(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn has_track_list(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn identity(&self) -> String {
        "gramtty".to_owned()
    }

    #[zbus(property)]
    fn supported_uri_schemes(&self) -> Vec<String> {
        Vec::new()
    }

    #[zbus(property)]
    fn supported_mime_types(&self) -> Vec<String> {
        Vec::new()
    }
}

struct Player {
    shared: Arc<Mutex<Shared>>,
    send: Callback,
}

impl Player {
    fn playing(&self) -> bool {
        lock(&self.shared).state.track.is_some()
    }
}

#[interface(name = "org.mpris.MediaPlayer2.Player")]
impl Player {
    fn play_pause(&self) {
        (self.send)(Command::PlayPause);
    }

    fn play(&self) {
        (self.send)(Command::Play);
    }

    fn pause(&self) {
        (self.send)(Command::Pause);
    }

    fn stop(&self) {
        (self.send)(Command::Stop);
    }

    fn next(&self) {
        (self.send)(Command::Next);
    }

    fn previous(&self) {
        (self.send)(Command::Previous);
    }

    fn seek(&self, offset: i64) {
        (self.send)(Command::Seek(offset));
    }

    fn set_position(&self, track_id: ObjectPath<'_>, position: i64) {
        let current = lock(&self.shared)
            .state
            .track
            .as_ref()
            .map(|t| t.path.clone());
        if current.as_deref() == Some(track_id.as_str()) {
            (self.send)(Command::SetPosition(position));
        }
    }

    #[zbus(signal)]
    async fn seeked(emitter: &SignalEmitter<'_>, position: i64) -> zbus::Result<()>;

    #[zbus(property)]
    fn playback_status(&self) -> String {
        status_str(lock(&self.shared).state.playback).to_owned()
    }

    #[zbus(property)]
    fn rate(&self) -> f64 {
        lock(&self.shared).state.rate
    }

    #[zbus(property)]
    fn minimum_rate(&self) -> f64 {
        1.0
    }

    #[zbus(property)]
    fn maximum_rate(&self) -> f64 {
        2.0
    }

    #[zbus(property)]
    fn metadata(&self) -> HashMap<String, OwnedValue> {
        metadata(lock(&self.shared).state.track.as_ref())
    }

    #[zbus(property)]
    fn volume(&self) -> f64 {
        lock(&self.shared).state.volume
    }

    #[zbus(property(emits_changed_signal = "false"))]
    fn position(&self) -> i64 {
        let g = lock(&self.shared);
        position_now(&g.state, g.at.elapsed())
    }

    #[zbus(property)]
    fn can_go_next(&self) -> bool {
        lock(&self.shared).state.can_next
    }

    #[zbus(property)]
    fn can_go_previous(&self) -> bool {
        lock(&self.shared).state.can_previous
    }

    #[zbus(property)]
    fn can_play(&self) -> bool {
        self.playing()
    }

    #[zbus(property)]
    fn can_pause(&self) -> bool {
        self.playing()
    }

    #[zbus(property)]
    fn can_seek(&self) -> bool {
        self.playing()
    }

    #[zbus(property)]
    fn can_control(&self) -> bool {
        true
    }
}

pub struct Handle {
    tx: UnboundedSender<State>,
    last: Option<State>,
}

impl Handle {
    pub fn push(&mut self, state: State) {
        if self.last.as_ref() != Some(&state) {
            self.last = Some(state.clone());
            let _ = self.tx.send(state);
        }
    }
}

pub fn spawn(
    address: Option<String>,
    send: impl Fn(Command) + Send + Sync + 'static,
    failed: impl FnOnce(String) + Send + 'static,
) -> Handle {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        if let Err(e) = run(address.as_deref(), Arc::new(send), rx).await {
            failed(e);
        }
    });
    Handle { tx, last: None }
}

fn describe(what: &str, e: &zbus::Error) -> String {
    format!("MPRIS is off, {what}: {e}")
}

async fn run(
    address: Option<&str>,
    send: Callback,
    mut rx: UnboundedReceiver<State>,
) -> Result<(), String> {
    let shared = Arc::new(Mutex::new(Shared {
        state: State::default(),
        at: Instant::now(),
    }));
    let builder = match address {
        Some(a) => connection::Builder::address(a),
        None => connection::Builder::session(),
    }
    .map_err(|e| describe("no session bus", &e))?;
    let conn = builder
        .serve_at(OBJECT_PATH, Root)
        .and_then(|b| {
            b.serve_at(
                OBJECT_PATH,
                Player {
                    shared: shared.clone(),
                    send,
                },
            )
        })
        .and_then(|b| b.name(BUS_NAME))
        .map_err(|e| describe("cannot set up the service", &e))?
        .build()
        .await
        .map_err(|e| describe("cannot claim the name or reach the bus", &e))?;
    let iref = conn
        .object_server()
        .interface::<_, Player>(OBJECT_PATH)
        .await
        .map_err(|e| describe("cannot reach the service", &e))?;
    while let Some(mut next) = rx.recv().await {
        while let Ok(newer) = rx.try_recv() {
            next = newer;
        }
        let old = {
            let mut g = lock(&shared);
            g.at = Instant::now();
            std::mem::replace(&mut g.state, next.clone())
        };
        let iface = iref.get().await;
        let ctx = iref.signal_emitter();
        let _ = emit_changes(&iface, ctx, &old, &next).await;
    }
    Ok(())
}

async fn emit_changes(
    iface: &Player,
    ctx: &SignalEmitter<'_>,
    old: &State,
    new: &State,
) -> zbus::Result<()> {
    if old.playback != new.playback {
        iface.playback_status_changed(ctx).await?;
    }
    if old.track != new.track {
        iface.metadata_changed(ctx).await?;
        if old.track.is_some() != new.track.is_some() {
            iface.can_play_changed(ctx).await?;
            iface.can_pause_changed(ctx).await?;
            iface.can_seek_changed(ctx).await?;
        }
    }
    if old.rate != new.rate {
        iface.rate_changed(ctx).await?;
    }
    if old.volume != new.volume {
        iface.volume_changed(ctx).await?;
    }
    if old.can_next != new.can_next {
        iface.can_go_next_changed(ctx).await?;
    }
    if old.can_previous != new.can_previous {
        iface.can_go_previous_changed(ctx).await?;
    }
    if old.seeks != new.seeks {
        Player::seeked(ctx, new.position_us).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::process::{Child, Stdio};

    fn track() -> Track {
        Track {
            path: track_path(-100, 7),
            title: "Song".to_owned(),
            artist: vec!["Artist".to_owned()],
            length_us: 190_000_000,
        }
    }

    #[test]
    fn playback_and_time_mappings() {
        assert_eq!(status_str(Playback::Playing), "Playing");
        assert_eq!(status_str(Playback::Paused), "Paused");
        assert_eq!(status_str(Playback::Stopped), "Stopped");
        assert_eq!(micros(1.5), 1_500_000);
        assert_eq!(micros(f64::NAN), 0);
        assert_eq!(seconds(2_500_000), 2.5);
        assert_eq!(track_path(-100, 7), "/org/gramtty/track/c100_m7");
        assert!(ObjectPath::try_from(track_path(-100, 7)).is_ok());
    }

    #[test]
    fn metadata_carries_track_id_length_title_and_artists() {
        let map = metadata(Some(&track()));
        let id: ObjectPath<'_> = map["mpris:trackid"].downcast_ref().unwrap();
        assert_eq!(id.as_str(), "/org/gramtty/track/c100_m7");
        assert_eq!(
            map["mpris:length"].downcast_ref::<i64>().unwrap(),
            190_000_000
        );
        assert_eq!(map["xesam:title"].downcast_ref::<&str>().unwrap(), "Song");
        let artists: Vec<String> = map["xesam:artist"].clone().try_into().unwrap();
        assert_eq!(artists, ["Artist"]);
        let none = metadata(None);
        assert_eq!(none.len(), 1);
        let id: ObjectPath<'_> = none["mpris:trackid"].downcast_ref().unwrap();
        assert_eq!(id.as_str(), NO_TRACK);
    }

    #[test]
    fn position_extrapolates_only_while_playing_and_stays_in_range() {
        let mut s = State {
            track: Some(track()),
            position_us: 10_000_000,
            rate: 2.0,
            playback: Playback::Playing,
            ..State::default()
        };
        assert_eq!(position_now(&s, Duration::from_secs(3)), 16_000_000);
        assert_eq!(position_now(&s, Duration::from_secs(900)), 190_000_000);
        s.playback = Playback::Paused;
        assert_eq!(position_now(&s, Duration::from_secs(3)), 10_000_000);
    }

    struct Daemon(Child);

    impl Drop for Daemon {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    fn private_bus() -> Option<(Daemon, String)> {
        let mut child = std::process::Command::new("dbus-daemon")
            .args(["--session", "--print-address", "--nofork"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let out = child.stdout.take()?;
        let daemon = Daemon(child);
        let mut line = String::new();
        BufReader::new(out).read_line(&mut line).ok()?;
        let address = line.trim().to_owned();
        (!address.is_empty()).then_some((daemon, address))
    }

    async fn eventually<T, F, Fut>(mut probe: F) -> Option<T>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Option<T>>,
    {
        for _ in 0..100 {
            if let Some(v) = probe().await {
                return Some(v);
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        None
    }

    #[tokio::test]
    async fn serves_status_and_commands_on_a_private_bus() {
        let Some((_daemon, address)) = private_bus() else {
            return;
        };
        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::unbounded_channel();
        let (fail_tx, mut fail_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut handle = spawn(
            Some(address.clone()),
            move |c| {
                let _ = cmd_tx.send(c);
            },
            move |e| {
                let _ = fail_tx.send(e);
            },
        );
        let client = connection::Builder::address(address.as_str())
            .unwrap()
            .build()
            .await
            .unwrap();
        let player = zbus::proxy::Builder::<zbus::Proxy>::new(&client)
            .destination(BUS_NAME)
            .unwrap()
            .path(OBJECT_PATH)
            .unwrap()
            .interface("org.mpris.MediaPlayer2.Player")
            .unwrap()
            .cache_properties(zbus::proxy::CacheProperties::No)
            .build()
            .await
            .unwrap();
        let status =
            eventually(|| async { player.get_property::<String>("PlaybackStatus").await.ok() })
                .await;
        assert_eq!(
            status.as_deref(),
            Some("Stopped"),
            "{:?}",
            fail_rx.try_recv()
        );
        let root = zbus::Proxy::new(&client, BUS_NAME, OBJECT_PATH, "org.mpris.MediaPlayer2")
            .await
            .unwrap();
        assert_eq!(
            root.get_property::<String>("Identity").await.unwrap(),
            "gramtty"
        );
        assert!(!root.get_property::<bool>("CanQuit").await.unwrap());
        assert!(!root.get_property::<bool>("HasTrackList").await.unwrap());
        assert_eq!(
            player.get_property::<f64>("MaximumRate").await.unwrap(),
            2.0
        );
        assert!(player.get_property::<bool>("CanControl").await.unwrap());
        assert!(!player.get_property::<bool>("CanPlay").await.unwrap());

        player.call_method("PlayPause", &()).await.unwrap();
        player.call_method("Seek", &(5_000_000i64,)).await.unwrap();
        assert_eq!(cmd_rx.recv().await, Some(Command::PlayPause));
        assert_eq!(cmd_rx.recv().await, Some(Command::Seek(5_000_000)));

        handle.push(State {
            playback: Playback::Playing,
            track: Some(track()),
            rate: 1.5,
            position_us: 42_000_000,
            can_next: true,
            ..State::default()
        });
        let status = eventually(|| async {
            let s = player.get_property::<String>("PlaybackStatus").await.ok()?;
            (s == "Playing").then_some(s)
        })
        .await;
        assert_eq!(status.as_deref(), Some("Playing"));
        assert_eq!(player.get_property::<f64>("Rate").await.unwrap(), 1.5);
        assert!(player.get_property::<bool>("CanGoNext").await.unwrap());
        let pos = player.get_property::<i64>("Position").await.unwrap();
        assert!((42_000_000..45_000_000).contains(&pos), "{pos}");
        let meta = player
            .get_property::<HashMap<String, OwnedValue>>("Metadata")
            .await
            .unwrap();
        assert_eq!(meta["xesam:title"].downcast_ref::<&str>().unwrap(), "Song");

        let wrong = ObjectPath::try_from("/org/gramtty/track/other").unwrap();
        player
            .call_method("SetPosition", &(wrong, 1_000_000i64))
            .await
            .unwrap();
        let right = ObjectPath::try_from(track_path(-100, 7)).unwrap();
        player
            .call_method("SetPosition", &(right, 2_000_000i64))
            .await
            .unwrap();
        assert_eq!(cmd_rx.recv().await, Some(Command::SetPosition(2_000_000)));
        assert!(cmd_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn an_unreachable_bus_reports_failure_instead_of_panicking() {
        let (cmd_tx, _cmd_rx) = tokio::sync::mpsc::unbounded_channel::<Command>();
        let (fail_tx, mut fail_rx) = tokio::sync::mpsc::unbounded_channel();
        let _handle = spawn(
            Some("unix:path=/nonexistent/gramtty-test-bus".to_owned()),
            move |c| {
                let _ = cmd_tx.send(c);
            },
            move |e| {
                let _ = fail_tx.send(e);
            },
        );
        let reason = tokio::time::timeout(Duration::from_secs(5), fail_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(reason.starts_with("MPRIS is off"), "{reason}");
    }
}
