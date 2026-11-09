use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::oneshot;

pub const RATE: u32 = 48_000;
pub const WAVEFORM_LEN: usize = 100;
pub const MIN_SAMPLES: usize = (RATE / 2) as usize;
pub const MAX_SECONDS: u64 = 15 * 60;
const ENCODE_TIMEOUT: Duration = Duration::from_secs(60);
pub const LEVEL_SAMPLES: usize = RATE as usize / 20;
const LEVEL_FLOOR_DB: f32 = 48.0;
const LIVE_BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

pub type Levels = Arc<Mutex<Vec<u16>>>;

#[derive(Debug, Default)]
pub struct LevelMeter {
    carry: Option<u8>,
    count: usize,
    peak: u16,
}

impl LevelMeter {
    pub fn feed(&mut self, bytes: &[u8], out: &mut Vec<u16>) {
        let mut it = bytes.iter().copied();
        while let Some(lo) = self.carry.take().or_else(|| it.next()) {
            let Some(hi) = it.next() else {
                self.carry = Some(lo);
                break;
            };
            self.peak = self.peak.max(i16::from_le_bytes([lo, hi]).unsigned_abs());
            self.count += 1;
            if self.count == LEVEL_SAMPLES {
                out.push(self.peak);
                self.count = 0;
                self.peak = 0;
            }
        }
    }
}

pub fn level_index(peak: u16) -> usize {
    if peak == 0 {
        return 0;
    }
    let db = 20.0 * (f32::from(peak) / f32::from(i16::MAX)).log10();
    let frac = ((db + LEVEL_FLOOR_DB) / LEVEL_FLOOR_DB).clamp(0.0, 1.0);
    (frac * (LIVE_BARS.len() - 1) as f32).round() as usize
}

pub fn live_tail(levels: &[u16], width: usize) -> &[u16] {
    &levels[levels.len().saturating_sub(width)..]
}

pub fn live_bars(levels: &[u16], width: usize) -> String {
    live_tail(levels, width)
        .iter()
        .map(|p| LIVE_BARS[level_index(*p)])
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VoiceNote {
    pub path: PathBuf,
    pub duration: i32,
    pub waveform: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoder {
    Opusenc,
    Ffmpeg,
}

pub fn find_in_path(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join(program))
        .find(|p| {
            use std::os::unix::fs::PermissionsExt;
            p.metadata()
                .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
}

pub fn pick_encoder(has: impl Fn(&str) -> bool) -> Result<Encoder, String> {
    if has("opusenc") {
        Ok(Encoder::Opusenc)
    } else if has("ffmpeg") {
        Ok(Encoder::Ffmpeg)
    } else {
        Err("voice messages need opusenc (opus-tools) or ffmpeg with libopus".to_owned())
    }
}

pub fn check_tools() -> Result<Encoder, String> {
    if find_in_path("pw-record").is_none() {
        return Err("voice messages need pw-record (PipeWire)".to_owned());
    }
    pick_encoder(|p| find_in_path(p).is_some())
}

pub fn record_args() -> Vec<String> {
    vec![
        "--raw".into(),
        "--rate".into(),
        RATE.to_string(),
        "--channels".into(),
        "1".into(),
        "--format".into(),
        "s16".into(),
        "--media-role".into(),
        "Communication".into(),
        "-".into(),
    ]
}

pub fn encoder_command(enc: Encoder, out: &Path) -> (&'static str, Vec<String>) {
    let out = out.to_string_lossy().into_owned();
    match enc {
        Encoder::Opusenc => (
            "opusenc",
            vec![
                "--quiet".into(),
                "--raw".into(),
                "--raw-bits".into(),
                "16".into(),
                "--raw-rate".into(),
                RATE.to_string(),
                "--raw-chan".into(),
                "1".into(),
                "--bitrate".into(),
                "32".into(),
                "-".into(),
                out,
            ],
        ),
        Encoder::Ffmpeg => (
            "ffmpeg",
            vec![
                "-v".into(),
                "error".into(),
                "-y".into(),
                "-f".into(),
                "s16le".into(),
                "-ar".into(),
                RATE.to_string(),
                "-ac".into(),
                "1".into(),
                "-i".into(),
                "pipe:0".into(),
                "-c:a".into(),
                "libopus".into(),
                "-b:a".into(),
                "32k".into(),
                "-application".into(),
                "voip".into(),
                "-f".into(),
                "ogg".into(),
                out,
            ],
        ),
    }
}

pub fn samples(pcm: &[u8]) -> Vec<i16> {
    pcm.as_chunks::<2>()
        .0
        .iter()
        .map(|c| i16::from_le_bytes(*c))
        .collect()
}

pub fn duration_secs(sample_count: usize) -> i32 {
    let secs = (sample_count as u64 + u64::from(RATE) / 2) / u64::from(RATE);
    secs.max(1).min(i32::MAX as u64) as i32
}

pub fn waveform(samples: &[i16]) -> Vec<u8> {
    if samples.is_empty() {
        return vec![0; WAVEFORM_LEN];
    }
    let peaks: Vec<u32> = (0..WAVEFORM_LEN)
        .map(|i| {
            let from = i * samples.len() / WAVEFORM_LEN;
            let to = ((i + 1) * samples.len() / WAVEFORM_LEN).max(from + 1);
            samples[from..to.min(samples.len())]
                .iter()
                .map(|s| s.unsigned_abs() as u32)
                .max()
                .unwrap_or(0)
        })
        .collect();
    let top = peaks.iter().copied().max().unwrap_or(0).max(1);
    peaks
        .into_iter()
        .map(|p| ((p * 31 + top / 2) / top).min(31) as u8)
        .collect()
}

pub fn pack_waveform(values: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; (values.len() * 5).div_ceil(8)];
    for (i, v) in values.iter().enumerate() {
        let v = u16::from(v & 31);
        let bit = i * 5;
        let shifted = v << (bit % 8);
        out[bit / 8] |= (shifted & 0xff) as u8;
        if shifted > 0xff {
            out[bit / 8 + 1] |= (shifted >> 8) as u8;
        }
    }
    out
}

pub fn is_ogg_opus(head: &[u8]) -> bool {
    head.starts_with(b"OggS") && head.windows(8).any(|w| w == b"OpusHead")
}

pub enum Stop {
    Send,
    Cancel,
}

pub struct Recorder {
    child: tokio::process::Child,
    reader: tokio::task::JoinHandle<std::io::Result<Vec<u8>>>,
    levels: Levels,
}

impl Recorder {
    pub fn start() -> Result<Self, String> {
        let mut child = tokio::process::Command::new("pw-record")
            .args(record_args())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| format!("pw-record: {e}"))?;
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| "pw-record: no stdout".to_owned())?;
        let levels = Levels::default();
        let shared = levels.clone();
        let reader = tokio::spawn(async move {
            let mut pcm = Vec::with_capacity(RATE as usize * 2 * 10);
            let mut chunk = vec![0u8; LEVEL_SAMPLES * 2];
            let mut meter = LevelMeter::default();
            let mut fresh = Vec::new();
            loop {
                let n = stdout.read(&mut chunk).await?;
                if n == 0 {
                    break;
                }
                pcm.extend_from_slice(&chunk[..n]);
                meter.feed(&chunk[..n], &mut fresh);
                if !fresh.is_empty() {
                    shared
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .append(&mut fresh);
                }
            }
            Ok(pcm)
        });
        Ok(Self {
            child,
            reader,
            levels,
        })
    }

    pub fn levels(&self) -> Levels {
        self.levels.clone()
    }

    pub async fn run(mut self, stop: oneshot::Receiver<Stop>) -> Result<Option<Vec<u8>>, String> {
        let limit = tokio::time::sleep(Duration::from_secs(MAX_SECONDS));
        let send = tokio::select! {
            status = self.child.wait() => {
                let status = status.map_err(|e| format!("pw-record: {e}"))?;
                return Err(format!("pw-record stopped unexpectedly ({status})"));
            }
            s = stop => matches!(s, Ok(Stop::Send)),
            _ = limit => true,
        };
        self.child
            .kill()
            .await
            .map_err(|e| format!("stop pw-record: {e}"))?;
        let pcm = self
            .reader
            .await
            .map_err(|e| format!("recording reader: {e}"))?
            .map_err(|e| format!("reading pw-record output: {e}"))?;
        Ok(send.then_some(pcm))
    }
}

pub async fn encode(pcm: Vec<u8>, enc: Encoder, uploads: &Path) -> Result<VoiceNote, String> {
    let s = samples(&pcm);
    if s.len() < MIN_SAMPLES {
        return Err("recording is too short".to_owned());
    }
    crate::upload::ensure_dir(uploads)?;
    let out = crate::upload::unique_upload_path(uploads, "voice", "ogg");
    let (program, args) = encoder_command(enc, &out);
    let mut child = tokio::process::Command::new(program)
        .args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("{program}: {e}"))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| format!("{program}: no stdin"))?;
    let writer = tokio::spawn(async move {
        stdin.write_all(&pcm).await?;
        stdin.shutdown().await
    });
    let output = tokio::time::timeout(ENCODE_TIMEOUT, child.wait_with_output())
        .await
        .map_err(|_| format!("{program} timed out"))?
        .map_err(|e| format!("{program}: {e}"))?;
    writer
        .await
        .map_err(|e| format!("{program} writer: {e}"))?
        .map_err(|e| format!("{program} input: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "{program} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let mut head = vec![0u8; 64];
    let mut f = tokio::fs::File::open(&out)
        .await
        .map_err(|e| format!("encoded voice: {e}"))?;
    let n = f
        .read(&mut head)
        .await
        .map_err(|e| format!("encoded voice: {e}"))?;
    if !is_ogg_opus(&head[..n]) {
        return Err(format!("{program} did not produce OGG/Opus"));
    }
    Ok(VoiceNote {
        path: out,
        duration: duration_secs(s.len()),
        waveform: waveform(&s),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::decode_waveform;

    #[test]
    fn waveform_packs_five_bits_and_roundtrips() {
        let values: Vec<u8> = (0..100).map(|i| (i * 7 % 32) as u8).collect();
        let packed = pack_waveform(&values);
        assert_eq!(packed.len(), 63);
        assert_eq!(decode_waveform(&packed), values);
        assert_eq!(pack_waveform(&[31, 15, 0]), vec![0xff, 0x01]);
        assert_eq!(
            decode_waveform(&pack_waveform(&[31, 15, 0]))[..3],
            [31, 15, 0]
        );
        assert_eq!(pack_waveform(&[255]), vec![31]);
    }

    #[test]
    fn waveform_from_samples_is_normalized() {
        let mut s = vec![0i16; 48_000];
        for (i, v) in s.iter_mut().enumerate() {
            *v = if i < 24_000 { 1000 } else { -i16::MAX };
        }
        let w = waveform(&s);
        assert_eq!(w.len(), WAVEFORM_LEN);
        assert_eq!(w[0], 1);
        assert_eq!(w[99], 31);
        assert!(w.iter().all(|v| *v <= 31));
        assert_eq!(waveform(&[]), vec![0; WAVEFORM_LEN]);
        assert_eq!(waveform(&[0, 0, 0]), vec![0; WAVEFORM_LEN]);
        assert_eq!(samples(&[1, 0, 0xff, 0xff, 9]), vec![1, -1]);
    }

    #[test]
    fn duration_and_encoder_choice() {
        assert_eq!(duration_secs(96_000), 2);
        assert_eq!(duration_secs(100), 1);
        assert_eq!(duration_secs(48_000 * 3 + 30_000), 4);
        assert_eq!(pick_encoder(|_| true), Ok(Encoder::Opusenc));
        assert_eq!(pick_encoder(|p| p == "ffmpeg"), Ok(Encoder::Ffmpeg));
        assert!(pick_encoder(|_| false).unwrap_err().contains("opusenc"));
        let (prog, args) = encoder_command(Encoder::Ffmpeg, Path::new("/x/v.ogg"));
        assert_eq!(prog, "ffmpeg");
        assert!(args.windows(2).any(|w| w[0] == "-c:a" && w[1] == "libopus"));
        assert_eq!(args.last().map(String::as_str), Some("/x/v.ogg"));
        assert!(is_ogg_opus(b"OggS\0\x02........OpusHead\x01"));
        assert!(!is_ogg_opus(b"RIFF....WAVE"));
    }

    #[test]
    fn level_meter_emits_one_peak_per_chunk_across_split_reads() {
        let mut pcm = Vec::new();
        for i in 0..LEVEL_SAMPLES * 3 + 10 {
            let v: i16 = match i / LEVEL_SAMPLES {
                0 => 100,
                1 => -20_000,
                _ => 5,
            };
            pcm.extend_from_slice(&v.to_le_bytes());
        }
        let mut meter = LevelMeter::default();
        let mut out = Vec::new();
        for part in pcm.chunks(777) {
            meter.feed(part, &mut out);
        }
        assert_eq!(out, vec![100, 20_000, 5]);
        meter.feed(&[], &mut out);
        assert_eq!(out.len(), 3);
    }

    #[test]
    fn level_index_is_monotonic_and_bounded() {
        assert_eq!(level_index(0), 0);
        assert_eq!(level_index(1), 0);
        assert_eq!(level_index(u16::MAX), 7);
        assert_eq!(level_index(i16::MAX as u16), 7);
        let mut last = 0;
        for p in (0..=32_767u16).step_by(97) {
            let i = level_index(p);
            assert!(i >= last && i <= 7);
            last = i;
        }
        assert!(level_index(3000) > level_index(300));
    }

    #[test]
    fn live_bars_keep_newest_on_the_right() {
        let levels: Vec<u16> = (0..10).map(|i| if i < 7 { 0 } else { 32_767 }).collect();
        assert_eq!(live_tail(&levels, 4), &[0, 32_767, 32_767, 32_767]);
        assert_eq!(live_bars(&levels, 4), "▁███");
        assert_eq!(live_bars(&levels, 50).chars().count(), 10);
        assert!(live_bars(&levels, 50).ends_with('█'));
        assert_eq!(live_bars(&levels, 0), "");
        assert_eq!(live_bars(&[], 5), "");
    }

    fn scratch() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("gramtty-voice-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[tokio::test]
    async fn encodes_synthetic_pcm() {
        let Ok(enc) = pick_encoder(|p| find_in_path(p).is_some()) else {
            eprintln!("no opus encoder installed, skipping");
            return;
        };
        let pcm: Vec<u8> = (0..RATE as usize * 2)
            .flat_map(|i| {
                let v = ((i as f32 * 440.0 * std::f32::consts::TAU / RATE as f32).sin()
                    * 8000.0
                    * (i as f32 / (RATE as f32 * 2.0))) as i16;
                v.to_le_bytes()
            })
            .collect();
        let dir = scratch();
        let note = encode(pcm, enc, &dir).await.unwrap();
        assert_eq!(note.duration, 2);
        assert_eq!(note.waveform.len(), WAVEFORM_LEN);
        assert!(note.waveform[0] < note.waveform[99]);
        let head = std::fs::read(&note.path).unwrap();
        assert!(is_ogg_opus(&head));
        assert!(encode(vec![0; 100], enc, &dir).await.is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    #[ignore]
    async fn live_levels_update_while_recording() {
        let enc = check_tools().unwrap();
        let rec = Recorder::start().unwrap();
        let levels = rec.levels();
        let (tx, rx) = oneshot::channel();
        let task = tokio::spawn(rec.run(rx));
        let mut counts = Vec::new();
        for step in 1..=6 {
            tokio::time::sleep(Duration::from_millis(500)).await;
            let l = levels.lock().unwrap().clone();
            eprintln!(
                "t={:.1}s bars={} last={:?} live={}",
                step as f32 * 0.5,
                l.len(),
                live_tail(&l, 6),
                live_bars(&l, 30)
            );
            counts.push(l.len());
        }
        let _ = tx.send(Stop::Send);
        let pcm = task.await.unwrap().unwrap().unwrap();
        assert!(counts.windows(2).all(|w| w[1] > w[0]));
        let all = levels.lock().unwrap().clone();
        let distinct: std::collections::BTreeSet<u16> = all.iter().copied().collect();
        eprintln!(
            "total {} bars, {} distinct peaks, min {:?} max {:?}",
            all.len(),
            distinct.len(),
            all.iter().min(),
            all.iter().max()
        );
        assert!(all.len() >= 50);
        assert!(distinct.len() > 1);
        let dir = scratch();
        let note = encode(pcm, enc, &dir).await.unwrap();
        eprintln!(
            "file {} duration {} s, final waveform {}",
            note.path.display(),
            note.duration,
            crate::model::waveform_bars(&note.waveform, 50)
        );
        assert_eq!(note.duration, 3);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    #[ignore]
    async fn records_two_seconds_from_default_source() {
        let enc = check_tools().unwrap();
        let rec = Recorder::start().unwrap();
        let (tx, rx) = oneshot::channel();
        let task = tokio::spawn(rec.run(rx));
        tokio::time::sleep(Duration::from_secs(2)).await;
        let _ = tx.send(Stop::Send);
        let pcm = task.await.unwrap().unwrap().unwrap();
        let n = pcm.len() / 2;
        eprintln!("captured {n} samples ({:.2} s)", n as f64 / RATE as f64);
        assert!(n as f64 / f64::from(RATE) > 1.5);
        let dir = scratch();
        let note = encode(pcm, enc, &dir).await.unwrap();
        let head = std::fs::read(&note.path).unwrap();
        eprintln!(
            "encoder {enc:?}: {} bytes, duration {} s, waveform {:?}",
            head.len(),
            note.duration,
            crate::model::waveform_bars(&note.waveform, 50)
        );
        assert!(is_ogg_opus(&head));
        assert_eq!(note.duration, 2);
        assert_eq!(
            decode_waveform(&pack_waveform(&note.waveform)),
            note.waveform
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
