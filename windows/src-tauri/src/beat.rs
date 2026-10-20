// Mochi's dance follows the music: the tempo and the beat of what the speakers
// play, found from system output (WASAPI loopback on Windows, PipeWire on
// Linux). Controlled by "Dance to the beat", and only while Spotify plays.
//
// Nothing is kept or sent anywhere: the samples are read in memory, reduced to
// a one-number-per-10-ms "how much new sound started" curve, and dropped. The
// island gets `beat` events: the tempo, and the moment of one beat to count on
// from.

use std::collections::VecDeque;
#[cfg(target_os = "linux")]
use std::{
    io::Read,
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

#[cfg(any(windows, target_os = "linux"))]
use std::sync::Mutex;

use serde::Serialize;
#[cfg(any(windows, target_os = "linux"))]
use tauri::{AppHandle, Emitter, State};


pub const RATE: usize = 44_100;
#[cfg(windows)]
#[path = "beat_capture.rs"]
mod capture;

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Beat {
    pub bpm: f64,
    pub last_beat_ago: f64,
    pub confidence: f64,
}

/// Review tempo without bias periodically; isolated spikes cannot replace it.
#[derive(Default)]
struct TempoReview {
    last: usize,
    candidate: Option<(f64, usize)>,
    milestone_checked: [bool; 2],
    forced: bool,
}
impl TempoReview {
    fn due(&self, heard: usize) -> bool {
        self.forced || self.candidate.is_some() || heard.saturating_sub(self.last) >= RATE * 8 ||
            [RATE * 30, RATE * 60].iter().enumerate()
                .any(|(i, &threshold)| !self.milestone_checked[i] && heard >= threshold)
    }
    fn check_milestones(&mut self, heard: usize) {
        for (i, threshold) in [RATE * 30, RATE * 60].iter().enumerate() {
            if heard >= *threshold { self.milestone_checked[i] = true; }
        }
    }
    fn consider(&mut self, heard: usize, current: f64, confidence: f32, next: f64, quality: f32) -> bool {
        if !next.is_finite() || !quality.is_finite() || !(50.0..=220.0).contains(&next) ||
            (next - current).abs() / current < 0.05 || quality < confidence.max(0.45) * 1.1 {
            self.candidate = None;
            self.last = heard;
            return false;
        }
        let since = match self.candidate {
            Some((bpm, since)) if (bpm - next).abs() / bpm <= 0.03 => since,
            _ => heard,
        };
        self.candidate = Some((next, since));
        if heard.saturating_sub(since) < RATE * 2 { return false; }
        self.candidate = None;
        self.last = heard;
        true
    }
}

/// A bounded audio history, analysed off the capture thread. The library's
/// tempo path and beat grid jointly resolve tempo and actual beat positions.
pub struct BeatTracker {
    history: VecDeque<f32>,
    since_estimate: usize,
    tempo: Option<f64>,
    heard: usize,
    review: TempoReview,
}
impl Default for BeatTracker { fn default() -> Self { Self::new() } }
impl BeatTracker {
    pub fn new() -> Self { Self { history: VecDeque::new(), since_estimate: 0, tempo: None, heard: 0, review: TempoReview::default() } }
    pub fn request_review(&mut self) { self.review.forced = true; }
    pub fn push(&mut self, samples: &[i16]) -> Option<Beat> {
        self.history.extend(samples.iter().map(|v| *v as f32 / 32768.0));
        self.since_estimate += samples.len();
        self.heard += samples.len();
        while self.history.len() > RATE * 12 { self.history.pop_front(); }
        let early = self.heard < RATE * 6;
        let interval = if early { RATE / 2 } else { RATE };
        if self.history.len() < RATE * 5 / 2 || self.since_estimate < interval { return None; }
        self.since_estimate = 0;
        // No stale beat reports through a silent pause or disconnected stream.
        let recent = self.history.iter().rev().take(RATE / 2);
        if recent.clone().map(|v| v * v).sum::<f32>() < 1e-8 {
            self.review.candidate = None;
            self.review.last = self.heard;
            return None;
        }
        let peak = self.history.iter().map(|v| v.abs()).fold(0.0f32, f32::max);
        if peak < 1e-5 { return None; }
        // Analysis must not depend on Spotify's volume or the Windows mixer.
        let audio: Vec<f32> = self.history.iter().map(|v| *v * 0.8 / peak).collect();
        // Keep the metrical level stable within a recording. A soft prior
        // prevents a snare fill from turning 140 BPM into its 93 BPM rival;
        // it still allows real tempo drift. Track changes/seeks reset this.
        let options = timestretch::TempoTrackingOptions {
            prior_center_bpm: self.tempo.unwrap_or(120.0),
            prior_octave_sigma: if self.tempo.is_some() { 0.45 } else { 1.0 },
            ..Default::default()
        };
        let mut grid = timestretch::analysis::beat::detect_beats_with_options(&audio, RATE as u32, &options);
        if let Some(tempo) = self.tempo {
            if self.review.due(self.heard) {
                self.review.forced = false;
                self.review.check_milestones(self.heard);
                let alternative = timestretch::analysis::beat::detect_beats(&audio, RATE as u32);
                let timely = alternative.beats.last().is_some_and(|&last|
                    (0.0..=2.0 * RATE as f64).contains(&(audio.len() as f64 - last)));
                let quality = if alternative.phase_untrusted || !timely { 0.0 } else { alternative.confidence };
                if self.review.consider(self.heard, tempo, grid.confidence, alternative.bpm, quality) {
                    grid = alternative;
                }
            }
        }
        if grid.phase_untrusted || grid.confidence < if early { 0.5 } else { 0.45 } || grid.beats.len() < 3 ||
            !grid.bpm.is_finite() || !(50.0..=220.0).contains(&grid.bpm) { return None; }
        let last = *grid.beats.last()?;
        let ago = (audio.len() as f64 - last) / RATE as f64;
        if !(0.0..=2.0).contains(&ago) { return None; }
        self.tempo = Some(grid.bpm);
        Some(Beat { bpm: grid.bpm, last_beat_ago: ago, confidence: grid.confidence as f64 * 10.0 })
    }
}

// ── Listening ─────────────────────────────────────────────────────────────────

#[cfg(target_os = "linux")]
#[derive(Default)]
pub struct Listener {
    run: Mutex<Option<Running>>,
}

/// Windows captures Spotify's process tree only, never a microphone.
#[cfg(windows)]
#[derive(Default)]
pub struct Listener {
    run: Mutex<Option<Running>>,
}

#[cfg(windows)]
struct Running {
    capture: Option<capture::Capture>,
    active: std::sync::Arc<std::sync::atomic::AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
    review_requested: std::sync::Arc<std::sync::atomic::AtomicBool>,
}
#[cfg(windows)]
impl Drop for Running {
    fn drop(&mut self) {
        self.active.store(false, std::sync::atomic::Ordering::Relaxed);
        self.capture.take();
        if let Some(worker) = self.worker.take() { let _ = worker.join(); }
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
#[derive(Default)]
pub struct Listener;

#[cfg(target_os = "linux")]
struct Running {
    stop: Arc<AtomicBool>,
    child: Arc<Mutex<Option<Child>>>,
}

#[cfg(target_os = "linux")]
fn stop_running(running: Running) {
    running.stop.store(true, Ordering::Relaxed);
    if let Some(mut child) = running.child.lock().unwrap().take() {
        let _ = child.kill();
        let _ = child.wait();
    }
}

#[cfg(any(windows, target_os = "linux"))]
fn now_ms() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64() * 1000.0)
        .unwrap_or(0.0)
}

/// Spotify plays and the user wants the dance on the beat: start listening;
/// otherwise stop. Asked by the island whenever either changes. The setting is
/// checked here too, so no page can start the listening without it.
#[cfg(target_os = "linux")]
#[tauri::command]
pub fn beat_enable(app: AppHandle, shared: State<'_, crate::Shared>, listener: State<'_, Listener>, on: bool) -> bool {
    let config = shared.settings.lock().unwrap().clone();
    let allowed = config.beat_sync && !crate::game::performance_active();
    let mut slot = listener.run.lock().unwrap();
    if !(on && allowed) {
        if let Some(running) = slot.take() {
            stop_running(running);
        }
        return false;
    }
    if slot.is_some() {
        return true;
    }
    let stop = Arc::new(AtomicBool::new(false));
    let child_slot: Arc<Mutex<Option<Child>>> = Arc::new(Mutex::new(None));
    let mut command = Command::new("pw-record");
    if !config.beat_output.is_empty() {
        command.arg("--target").arg(&config.beat_output);
    }
    let spawned = command
        .args([
            "-P", "stream.capture.sink=true",
            "--rate", "44100",
            "--channels", "1",
            "--format", "s16",
            "--latency", "20ms",
            "-",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(_) => return false, // no PipeWire: the dance stays at its own tempo
    };
    let Some(mut out) = child.stdout.take() else {
        let _ = child.kill();
        return false;
    };
    *child_slot.lock().unwrap() = Some(child);
    *slot = Some(Running { stop: stop.clone(), child: child_slot });
    drop(slot);

    std::thread::spawn(move || {
        let mut tracker = BeatTracker::new();
        let mut buf = [0u8; 4096];
        let mut odd: Option<u8> = None;
        while !stop.load(Ordering::Relaxed) {
            let n = match out.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            let mut bytes: Vec<u8> = Vec::with_capacity(n + 1);
            if let Some(b) = odd.take() {
                bytes.push(b);
            }
            bytes.extend_from_slice(&buf[..n]);
            if bytes.len() % 2 == 1 {
                odd = bytes.pop();
            }
            let samples: Vec<i16> = bytes.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect();
            if let Some(beat) = tracker.push(&samples) {
                let _ = app.emit("beat", BeatEvent { bpm: beat.bpm, beat_at_ms: now_ms() - beat.last_beat_ago * 1000.0, confidence: beat.confidence });
            }
        }
    });
    true
}

#[cfg(any(windows, target_os = "linux"))]
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct BeatEvent {
    bpm: f64,
    /// Unix time in ms of a beat; the others follow every 60000 / bpm.
    beat_at_ms: f64,
    confidence: f64,
}

#[cfg(not(any(windows, target_os = "linux")))]
#[tauri::command]
pub fn beat_enable(_on: bool) -> bool {
    false
}

// Spotify's process tree only: no system mix and no microphone fallback.
#[cfg(windows)]
#[tauri::command]
pub fn beat_enable(app: AppHandle, shared: State<'_, crate::Shared>, listener: State<'_, Listener>, on: bool) -> bool {
    let config = shared.settings.lock().unwrap().clone();
    let allowed = config.beat_sync && !crate::game::performance_active();
    let mut slot = listener.run.lock().unwrap();
    if !(on && allowed) {
        // Join the old analysis before resetting both renderer clocks.
        slot.take();
        return false;
    }
    if slot.is_some() { return true; }
    let (tx, rx) = std::sync::mpsc::sync_channel(64);
    let mut lost = false;
    match capture::start(move |samples, end_ms, discontinuity| {
        lost = tx.try_send((samples.to_vec(), end_ms, discontinuity || lost)).is_err();
    }) {
        Ok(capture) => {
            let active = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
            let running = active.clone();
            let review_requested = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let worker_review = review_requested.clone();
            let worker = std::thread::spawn(move || {
                let mut tracker = BeatTracker::new();
                while running.load(std::sync::atomic::Ordering::Relaxed) {
                    let Ok((samples, end_ms, discontinuity)) = rx.recv() else { break };
                    if discontinuity { tracker = BeatTracker::new(); }
                    if worker_review.swap(false, std::sync::atomic::Ordering::Relaxed) {
                        tracker.request_review();
                    }
                    if let Some(beat) = tracker.push(&samples) {
                        if running.load(std::sync::atomic::Ordering::Relaxed) {
                            let _ = app.emit("beat", BeatEvent { bpm: beat.bpm, beat_at_ms: end_ms - beat.last_beat_ago * 1000.0, confidence: beat.confidence });
                        }
                    }
                }
            });
            *slot = Some(Running { capture: Some(capture), active, worker: Some(worker), review_requested });
            true
        }
        Err(err) => { crate::log::line(format!("beat: {err}")); false }
    }
}

/// Ask for an unbiased check at a Spotify song-position milestone.
#[cfg(windows)]
#[tauri::command]
pub fn beat_review(listener: State<'_, Listener>) {
    if let Ok(slot) = listener.run.lock() {
        if let Some(running) = slot.as_ref() {
            running.review_requested.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }
}

#[cfg(not(windows))]
#[tauri::command]
pub fn beat_review() {}

#[cfg(test)]
mod tests {
    use super::*;

    /// A click track: a short low thump on every beat, a hat between, and a
    /// little noise, `secs` long, the first beat at `first_beat` seconds.
    fn click_track(bpm: f64, secs: f64, first_beat: f64) -> Vec<i16> {
        let n = (secs * RATE as f64) as usize;
        let period = 60.0 / bpm;
        let mut seed = 12345u32;
        let mut noise = || {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            ((seed >> 16) as f32 / 65536.0 - 0.5) * 0.04
        };
        (0..n)
            .map(|i| {
                let t = i as f64 / RATE as f64;
                let since = (t - first_beat).rem_euclid(period);
                let kick = if since < 0.12 {
                    (-(since * 35.0)).exp() as f32 * (2.0 * std::f32::consts::PI * 70.0 * since as f32).sin() * 0.8
                } else {
                    0.0
                };
                let hat_since = (t - first_beat - period / 2.0).rem_euclid(period);
                let hat = if hat_since < 0.02 { 0.15 * (hat_since as f32 * 9000.0).sin() } else { 0.0 };
                ((kick + hat + noise()) * 30000.0) as i16
            })
            .collect()
    }

    fn run(bpm: f64, first_beat: f64) -> Beat {
        let secs = 12.0;
        let mut t = BeatTracker::new();
        let mut last = None;
        let mut count = 0;
        for chunk in click_track(bpm, secs, first_beat).chunks(2048) {
            count += chunk.len();
            if let Some(b) = t.push(chunk) {
                last = Some((b, count));
            }
        }
        let (mut beat, at) = last.expect("an estimate");
        beat.last_beat_ago += secs - at as f64 / RATE as f64;
        beat
    }

    #[test]
    fn clear_percussion_is_available_in_three_seconds_with_a_real_beat_phase() {
        for bpm in [100.0, 120.0, 128.0, 146.0] {
            let mut tracker = BeatTracker::new();
            let mut heard = 0;
            let mut first = None;
            for chunk in click_track(bpm, 4.0, 0.2).chunks(2048) {
                heard += chunk.len();
                if let Some(beat) = tracker.push(chunk) { first = Some(beat); break; }
            }
            assert!(heard <= RATE * 31 / 10, "{bpm}: first report took {} seconds", heard as f64 / RATE as f64);
            assert_on_the_beat(first.expect("early estimate"), bpm, 0.2, heard as f64 / RATE as f64);
            assert!(tracker.history.len() <= RATE * 12);
        }
    }

    fn assert_on_the_beat(beat: Beat, bpm: f64, first_beat: f64, at_secs: f64) {
        assert!((beat.bpm - bpm).abs() / bpm < 0.02, "wanted {bpm}, got {}", beat.bpm);
        // Where the real last beat was, compared with where the tracker says.
        let period = 60.0 / bpm;
        let real_ago = (at_secs - first_beat).rem_euclid(period);
        let diff = (beat.last_beat_ago - real_ago).abs();
        let diff = diff.min(period - diff);
        assert!(diff < 0.06, "off the beat by {diff}s (period {period})");
    }

    #[test]
    fn callback_size_does_not_shift_the_beat_timestamp() {
        for chunk_size in [333, 2048, 8191] {
            let mut tracker = BeatTracker::new();
            let mut count = 0;
            for chunk in click_track(128.0, 12.0, 0.27).chunks(chunk_size) {
                count += chunk.len();
                if let Some(beat) = tracker.push(chunk) {
                    if count > RATE * 8 {
                        assert_on_the_beat(beat, 128.0, 0.27, count as f64 / RATE as f64);
                    }
                }
            }
        }
    }

    #[test]
    fn phase_relocks_to_recent_beats_after_a_seek() {
        let mut tracker = BeatTracker::new();
        tracker.push(&click_track(120.0, 8.0, 0.3));
        let beat = tracker.push(&click_track(120.0, 4.0, 0.1)).unwrap();
        assert_on_the_beat(beat, 120.0, 8.1, 12.0);
    }

    #[test]
    fn windows_loopback_resampling_preserves_tempo_and_phase() {
        let input: Vec<f32> = click_track(128.0, 12.0, 0.25).iter().map(|v| *v as f32 / 32768.0).collect();
        let mut upsampled = Vec::new();
        crate::speech::audio::Resampler::new(RATE as u32).process(&input, &mut upsampled);
        let mut converter = crate::speech::audio::Resampler::with_output_rate(16_000, RATE as u32);
        let mut tracker = BeatTracker::new();
        let mut last = None;
        let mut converted = Vec::new();
        let mut count = 0;
        for frame in upsampled.chunks(480) {
            converted.clear();
            converter.process(frame, &mut converted);
            let mono: Vec<i16> = converted.iter().map(|v| (*v * 32767.0) as i16).collect();
            count += mono.len();
            if let Some(beat) = tracker.push(&mono) { last = Some((beat, count)); }
        }
        let (beat, at) = last.expect("loopback must produce a tempo estimate");
        assert_on_the_beat(beat, 128.0, 0.25, at as f64 / RATE as f64);
    }

    #[test]
    fn finds_120_bpm_and_where_the_beats_fall() {
        let beat = run(120.0, 0.3);
        // `run` advances the last estimate to the end of the test audio.
        assert_on_the_beat(beat, 120.0, 0.3, 12.0);
    }

    #[test]
    fn finds_a_slow_and_a_fast_tempo() {
        for bpm in [84.0, 100.0, 126.0, 146.0] {
            let b = run(bpm, 0.1);
            assert!((b.bpm - bpm).abs() / bpm < 0.03, "{bpm} -> {}", b.bpm);
        }
    }

    #[test]
    fn metrical_ambiguity_stays_on_a_related_pulse() {
        // Kick + offbeat hat deliberately permits half/double-time readings.
        // Audio alone cannot uniquely identify the composer's notated meter.
        for real in [70.0, 175.0, 200.0] {
            let b = run(real, 0.2);
            assert!([0.5, 1.0, 2.0].iter().any(|ratio|
                (b.bpm - real * ratio).abs() / (real * ratio) < 0.03), "{real} -> {}", b.bpm);
        }
    }

    #[test]
    fn silence_never_republishes_beats_from_old_audio() {
        let mut tracker = BeatTracker::new();
        assert!(tracker.push(&click_track(120.0, 8.0, 0.3)).is_some());
        assert!(tracker.push(&vec![0; RATE * 2]).is_none());
    }

    #[test]
    fn silence_and_steady_noise_have_no_beat() {
        let mut t = BeatTracker::new();
        assert!(t.push(&vec![0i16; RATE * 6]).is_none());
        let mut seed = 7u32;
        let noise: Vec<i16> = (0..RATE * 8)
            .map(|_| {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                ((seed >> 16) as i32 - 32768) as i16 / 8
            })
            .collect();
        let mut t = BeatTracker::new();
        assert!(t.push(&noise).is_none());
    }

    #[test]
    fn odd_chunk_sizes_make_no_difference() {
        let samples = click_track(120.0, 8.0, 0.3);
        let mut a = BeatTracker::new();
        let mut b = BeatTracker::new();
        let mut last_a = None;
        let mut last_b = None;
        for c in samples.chunks(4096) {
            if let Some(x) = a.push(c) {
                last_a = Some(x);
            }
        }
        for c in samples.chunks(333) {
            if let Some(x) = b.push(c) {
                last_b = Some(x);
            }
        }
        assert_eq!(last_a.map(|b| (b.bpm * 100.0).round()), last_b.map(|b| (b.bpm * 100.0).round()));
    }
}

#[cfg(test)]
mod review_tests {
    use super::{TempoReview, RATE};
    #[test]
    fn reviews_at_thirty_and_sixty_seconds_even_between_regular_checks() {
        let mut review = TempoReview::default();
        review.last = RATE * 27;
        assert!(!review.due(RATE * 29));
        assert!(review.due(RATE * 30));
        review.check_milestones(RATE * 30);
        assert!(!review.due(RATE * 31));
        review.last = RATE * 57;
        assert!(!review.due(RATE * 59));
        assert!(review.due(RATE * 60));
        review.check_milestones(RATE * 60);
        assert!(!review.due(RATE * 61));
    }
    #[test]
    fn a_better_tempo_must_remain_stable_for_two_seconds() {
        let mut review = TempoReview::default();
        assert!(!review.due(RATE * 7));
        assert!(review.due(RATE * 8));
        assert!(!review.consider(RATE * 30, 112.0, 0.6, 128.0, 0.8));
        assert!(review.due(RATE * 31));
        assert!(!review.consider(RATE * 31, 112.0, 0.6, 128.2, 0.8));
        assert!(review.consider(RATE * 38, 112.0, 0.6, 128.0, 0.8));
        assert!(!review.due(RATE * 39));
    }
    #[test]
    fn weaker_or_brief_alternatives_never_replace_the_current_tempo() {
        let mut review = TempoReview::default();
        assert!(!review.consider(RATE * 30, 112.0, 0.8, 128.0, 0.6));
        assert!(!review.consider(RATE * 60, 112.0, 0.6, 128.0, 0.8));
        assert!(!review.consider(RATE * 61, 112.0, 0.6, 112.0, 0.8));
        assert!(!review.consider(RATE * 68, 112.0, 0.6, 128.0, 0.8));
    }
    #[test]
    fn changing_candidates_restart_the_confirmation_period() {
        let mut review = TempoReview::default();
        assert!(!review.consider(RATE * 30, 112.0, 0.6, 128.0, 0.8));
        assert!(!review.consider(RATE * 34, 112.0, 0.6, 150.0, 0.8));
        assert!(!review.consider(RATE * 35, 112.0, 0.6, 150.0, 0.8));
        assert!(review.consider(RATE * 42, 112.0, 0.6, 150.0, 0.8));
    }
}

#[cfg(test)]
mod live {
    use super::*;
    /// Local check against a real capture: COUCOU_BEAT_FILE=/path cargo test live -- --ignored --nocapture
    #[test]
    #[ignore]
    fn real_capture() {
        let Ok(path) = std::env::var("COUCOU_BEAT_FILE") else { return };
        let bytes = std::fs::read(path).unwrap();
        let samples: Vec<i16> = bytes.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect();
        let mut t = BeatTracker::new();
        for chunk in samples.chunks(2048) {
            if let Some(b) = t.push(chunk) {
                eprintln!("bpm {:.1} conf {:.1} last beat {:.3}s ago", b.bpm, b.confidence, b.last_beat_ago);
            }
        }
    }
}

/// Stop audio capture and beat analysis before yielding to the game.
#[cfg(windows)]
pub fn performance_suspend(app: &AppHandle) {
    use tauri::Manager;
    if let Some(listener) = app.try_state::<Listener>() {
        listener.run.lock().unwrap().take();
    }
}
