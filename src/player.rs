use cursive::{
    theme::{Effect, Style},
    utils::markup::StyledString,
};
use rayon::prelude::*;
use rodio::{Decoder, DeviceSinkBuilder, MixerDeviceSink, Player, Source, source::Buffered};
use std::{
    error::Error,
    fmt,
    fs::File,
    io::BufReader,
    path::{Path, PathBuf},
    str::FromStr,
    sync::mpsc::{Receiver, channel},
    thread,
    time::{Duration, Instant},
};

use crate::utils;

type PreloadedSource = Buffered<Decoder<BufReader<File>>>;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum RepeatMode {
    Off,
    Single,
    Album,
    All,
}

impl RepeatMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            RepeatMode::Off => "off",
            RepeatMode::Single => "single",
            RepeatMode::Album => "album",
            RepeatMode::All => "all",
        }
    }
}

impl FromStr for RepeatMode {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "off" => Ok(RepeatMode::Off),
            "single" => Ok(RepeatMode::Single),
            "album" => Ok(RepeatMode::Album),
            "all" => Ok(RepeatMode::All),
            _ => Err(()),
        }
    }
}

impl fmt::Display for RepeatMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

#[derive(Clone, Debug)]
pub(crate) struct TrackMetadata {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_artist: String,
    pub year: String,
    pub track_number: u16,
    pub duration: Duration,
}

impl fmt::Display for TrackMetadata {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let fields: [(&str, String); 7] = [
            ("Title", self.title.clone()),
            ("Artist", self.artist.clone()),
            ("Album Artist", self.album_artist.clone()),
            ("Album", self.album.clone()),
            ("Year", self.year.to_string()),
            ("Track Number", self.track_number.to_string()),
            ("Duration", utils::format_time(self.duration.as_secs() as usize)),
        ];

        for (i, (label, val)) in fields.iter().enumerate() {
            if i > 0 {
                writeln!(f)?;
            }
            write!(f, "{:<14} {}", format!("{}:", label), val)?;
        }

        Ok(())
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Track {
    pub metadata: TrackMetadata,
    pub path: PathBuf,
}

pub(crate) struct MusicPlayer {
    _handle: MixerDeviceSink,
    pub player: Player,
    pub queue: Vec<Track>,
    pub current_index: usize,
    pub track_start_time: Instant,
    pub is_paused: bool,
    pub paused_elapsed: Duration,
    pub repeat_mode: RepeatMode,

    // Pre-loading state
    preload_rx: Option<Receiver<Result<(usize, PreloadedSource), String>>>,
    preloaded_track: Option<(usize, PreloadedSource)>,
}

impl MusicPlayer {
    pub(crate) fn new(queue: Vec<Track>, allowed_base_dir: Option<&Path>, repeat_mode: RepeatMode) -> Result<Self, Box<dyn Error>> {
        if let Some(base) = allowed_base_dir {
            let canonical_base = base.canonicalize()?;
            queue
                .par_iter()
                .try_for_each(|track| -> Result<(), Box<dyn Error + Send + Sync>> {
                    let canonical_path = track.path.canonicalize()?;
                    if !canonical_path.starts_with(&canonical_base) {
                        return Err(format!("Security error: Path traversal detected: {:?}", track.path).into());
                    }
                    Ok(())
                })
                .map_err(|e| -> Box<dyn Error> { e })?;
        }

        let handle = DeviceSinkBuilder::open_default_sink()?;
        let player = Player::connect_new(&handle.mixer());

        let mut instance = Self {
            _handle: handle,
            player,
            queue,
            current_index: 0,
            track_start_time: Instant::now(),
            is_paused: false,
            paused_elapsed: Duration::ZERO,
            repeat_mode,
            preload_rx: None,
            preloaded_track: None,
        };

        if !instance.queue.is_empty() {
            instance.preload_track(0);
        }

        Ok(instance)
    }

    fn preload_track(&mut self, index: usize) {
        let Some(track) = self.queue.get(index).cloned() else {
            return;
        };

        let (tx, rx) = channel();
        self.preload_rx = Some(rx);

        thread::spawn(move || {
            let result = File::open(&track.path)
                .map_err(|e| e.to_string())
                .and_then(|file| Decoder::try_from(BufReader::new(file)).map_err(|e| e.to_string()))
                .map(|decoder| (index, decoder.buffered()));

            let _ = tx.send(result);
        });
    }

    fn poll_preloaded(&mut self) {
        if let Some(ref rx) = self.preload_rx {
            if let Ok(result) = rx.try_recv() {
                if let Ok(data) = result {
                    self.preloaded_track = Some(data);
                }
                self.preload_rx = None;
            }
        }
    }

    pub(crate) fn play_index(&mut self, index: usize, start_offset: Duration, start_paused: bool) -> Result<(), Box<dyn Error>> {
        if index >= self.queue.len() {
            return Err("Index out of bounds".into());
        }

        self.current_index = index;
        self.player.stop();
        self.poll_preloaded();

        if let Some(track) = self.queue.get(index) {
            let file = File::open(&track.path)?;
            let source = Decoder::try_from(BufReader::new(file))?;
            self.player.append(source);
        }

        if !start_offset.is_zero() {
            let _ = self.player.try_seek(start_offset);
        }

        self.track_start_time = Instant::now() - start_offset;
        self.paused_elapsed = start_offset;

        if start_paused {
            self.player.pause();
            self.is_paused = true;
        } else {
            self.player.play();
            self.is_paused = false;
        }

        let next_index = self.next_index(false);
        self.preload_track(next_index);

        Ok(())
    }

    fn next_index(&self, is_manual_skip: bool) -> usize {
        if self.queue.is_empty() {
            return 0;
        }

        match self.repeat_mode {
            RepeatMode::Single => {
                if is_manual_skip {
                    (self.current_index + 1) % self.queue.len()
                } else {
                    self.current_index
                }
            }
            RepeatMode::Album => {
                let (start, end) = self.album_bounds(self.current_index);
                if self.current_index >= end { start } else { self.current_index + 1 }
            }
            RepeatMode::All => (self.current_index + 1) % self.queue.len(),
            RepeatMode::Off => (self.current_index + 1).min(self.queue.len() - 1),
        }
    }

    fn album_bounds(&self, index: usize) -> (usize, usize) {
        if self.queue.is_empty() || index >= self.queue.len() {
            return (0, 0);
        }
        let current_track = &self.queue[index];
        let cur_artist = if current_track.metadata.album_artist.is_empty() {
            &current_track.metadata.artist
        } else {
            &current_track.metadata.album_artist
        };
        let cur_album = &current_track.metadata.album;

        let mut start = index;
        while start > 0 {
            let t = &self.queue[start - 1];
            let artist = if t.metadata.album_artist.is_empty() { &t.metadata.artist } else { &t.metadata.album_artist };
            if artist == cur_artist && &t.metadata.album == cur_album {
                start -= 1;
            } else {
                break;
            }
        }

        let mut end = index;
        while end + 1 < self.queue.len() {
            let t = &self.queue[end + 1];
            let artist = if t.metadata.album_artist.is_empty() { &t.metadata.artist } else { &t.metadata.album_artist };
            if artist == cur_artist && &t.metadata.album == cur_album {
                end += 1;
            } else {
                break;
            }
        }

        (start, end)
    }

    pub(crate) fn skip(&mut self) {
        if self.queue.is_empty() {
            return;
        }
        let next = self.next_index(true);
        let _ = self.play_index(next, Duration::ZERO, false);
    }

    pub(crate) fn previous(&mut self) -> Result<(), Box<dyn Error>> {
        if self.queue.is_empty() {
            return Ok(());
        }

        let new_index = if self.repeat_mode == RepeatMode::Single || self.repeat_mode == RepeatMode::All {
            if self.current_index == 0 { self.queue.len() - 1 } else { self.current_index - 1 }
        } else {
            self.current_index.saturating_sub(1)
        };

        self.play_index(new_index, Duration::ZERO, false)
    }

    pub(crate) fn advance_track(&mut self) {
        if self.queue.is_empty() {
            return;
        }
        let next = self.next_index(false);
        let _ = self.play_index(next, Duration::ZERO, false);
    }

    pub(crate) fn play_pause(&mut self) {
        if self.player.is_paused() {
            self.player.play();
            if self.is_paused {
                self.track_start_time = Instant::now() - self.paused_elapsed;
                self.is_paused = false;
            }
        } else {
            self.player.pause();
            if !self.is_paused {
                self.paused_elapsed = self.track_start_time.elapsed();
                self.is_paused = true;
            }
        }
    }

    pub(crate) fn toggle_repeat_mode(&mut self) {
        self.repeat_mode = match self.repeat_mode {
            RepeatMode::Off => RepeatMode::Single,
            RepeatMode::Single => RepeatMode::Album,
            RepeatMode::Album => RepeatMode::All,
            RepeatMode::All => RepeatMode::Off,
        };
    }

    pub(crate) fn jump_to(&mut self, index: usize) -> Result<(), Box<dyn Error>> {
        self.play_index(index, Duration::ZERO, false)
    }

    fn get_elapsed(&self) -> Duration {
        if self.is_paused { self.paused_elapsed } else { self.track_start_time.elapsed() }
    }

    fn seek_relative(&mut self, seconds: f64) {
        let Some(track) = self.queue.get(self.current_index) else {
            return;
        };

        let current_pos = self.get_elapsed();
        let total_duration = track.metadata.duration;

        if seconds >= 0.0 {
            let offset = Duration::from_secs_f64(seconds);
            let target_pos = (current_pos + offset).min(total_duration);
            if self.player.try_seek(target_pos).is_ok() {
                self.update_timer_state(target_pos);
            } else {
                let _ = self.play_index(self.current_index, target_pos, self.is_paused);
            }
        } else {
            let offset = Duration::from_secs_f64(seconds.abs());
            let target_pos = current_pos.saturating_sub(offset);
            let _ = self.play_index(self.current_index, target_pos, self.is_paused);
        }
    }

    fn update_timer_state(&mut self, target_pos: Duration) {
        if self.is_paused {
            self.paused_elapsed = target_pos;
        } else {
            self.track_start_time = Instant::now() - target_pos;
        }
    }

    // Fast forwards a track by 10 seconds.
    pub(crate) fn seek_forward(&mut self) {
        self.seek_relative(10.0);
    }

    // Rewinds a track by 10 seconds.
    pub(crate) fn seek_backward(&mut self) {
        self.seek_relative(-10.0);
    }

    pub(crate) fn current_track_info(&self) -> StyledString {
        if let Some(track) = self.queue.get(self.current_index) {
            let track = &track.metadata;
            let mut styled = StyledString::new();
            let bold = Style::from(Effect::Bold);
            styled.append_styled(&track.title, bold);
            styled.append_plain(" by ");
            styled.append_styled(&track.artist, bold);
            styled.append_plain(" on ");
            styled.append_styled(&track.album, bold);
            styled.append_plain(format!(" ({})", track.year));
            styled
        } else if self.queue.is_empty() {
            StyledString::plain("No tracks loaded")
        } else {
            StyledString::plain("Playback Finished")
        }
    }

    pub(crate) fn get_current_progress(&self) -> (usize, usize) {
        let Some(track) = self.queue.get(self.current_index) else {
            return (0, 100);
        };

        let total_secs = track.metadata.duration.as_secs() as usize;
        if total_secs == 0 {
            return (0, 100);
        }

        let elapsed = self.get_elapsed();
        let current_secs = elapsed.as_secs() as usize;

        (current_secs.min(total_secs), total_secs)
    }
}
