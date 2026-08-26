use cursive::{
    theme::{Effect, Style},
    utils::markup::StyledString,
};
use rodio::{Decoder, DeviceSinkBuilder, MixerDeviceSink, Player};
use std::{
    error::Error,
    fmt,
    fs::File,
    io::BufReader,
    path::PathBuf,
    str::FromStr,
    sync::mpsc::{Receiver, Sender, channel},
    thread,
    time::{Duration, Instant},
};

use crate::utils;

/// Type alias for preloaded audio decoders to avoid using `rodio::source::Buffered`,
/// keeping memory usage lightweight by streaming directly from disk.
type PreloadedSource = Decoder<BufReader<File>>;

/// Request sent to the background decoding worker thread.
struct PreloadRequest {
    generation: u64,
    index: usize,
    path: PathBuf,
}

/// Supported playback repeat behaviors.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum RepeatMode {
    Off,
    Single,
    Album,
    All,
}

impl RepeatMode {
    /// Returns the string representation of the repeat mode.
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

    /// Parses a string into a corresponding `RepeatMode`.
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
        f.write_str(self.as_str())
    }
}

/// Metadata describing a single audio track.
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
        writeln!(f, "{:<14} {}", "Title:", self.title)?;
        writeln!(f, "{:<14} {}", "Artist:", self.artist)?;
        writeln!(f, "{:<14} {}", "Album Artist:", self.album_artist)?;
        writeln!(f, "{:<14} {}", "Album:", self.album)?;
        writeln!(f, "{:<14} {}", "Year:", self.year)?;
        writeln!(f, "{:<14} {}", "Track Number:", self.track_number)?;
        write!(f, "{:<14} {}", "Duration:", utils::format_time(self.duration.as_secs() as usize))
    }
}

/// Represents a playable track entry in the audio queue.
#[derive(Clone, Debug)]
pub(crate) struct Track {
    pub metadata: TrackMetadata,
    pub path: PathBuf,
    /// Cached index range `(start, end)` of the album this track belongs to.
    pub album_range: (usize, usize),
}

impl Track {
    /// Creates a new `Track` instance with default zeroed album boundaries.
    pub fn new(metadata: TrackMetadata, path: PathBuf) -> Self {
        Self { metadata, path, album_range: (0, 0) }
    }
}

/// Manages audio output state, track queues, preloading, and user playback controls.
pub(crate) struct MusicPlayer {
    _handle: MixerDeviceSink,
    pub player: Player,
    pub queue: Vec<Track>,
    pub current_index: usize,
    pub track_start_time: Instant,
    pub is_paused: bool,
    pub paused_elapsed: Duration,
    pub repeat_mode: RepeatMode,

    // Channels and state for managing the persistent background decoding worker
    preload_generation: u64,
    preload_tx: Sender<PreloadRequest>,
    preload_rx: Receiver<(u64, usize, Result<PreloadedSource, String>)>,
    preloaded_track: Option<(usize, PreloadedSource)>,
}

impl MusicPlayer {
    /// Initializes the music player, validates security paths, pre-computes album ranges,
    /// and spins up the persistent background decoding thread.
    pub(crate) fn new(mut queue: Vec<Track>, repeat_mode: RepeatMode) -> Result<Self, Box<dyn Error + Send + Sync>> {
        // Pre-compute album ranges once during initialization for O(1) lookups during playback
        Self::assign_album_ranges(&mut queue);

        // Open the default sink and connect the Player to its mixer
        let handle = DeviceSinkBuilder::open_default_sink()?;
        let player = Player::connect_new(&handle.mixer());

        // Setup channel communication for the long-lived background worker thread
        let (req_tx, req_rx) = channel::<PreloadRequest>();
        let (res_tx, res_rx) = channel();

        // Worker thread loop: listens for requests and decodes audio in the background
        thread::spawn(move || {
            while let Ok(req) = req_rx.recv() {
                // Drain any pending backlog requests in the queue to skip outdated skips
                let mut latest_req = req;
                while let Ok(newer_req) = req_rx.try_recv() {
                    latest_req = newer_req;
                }

                let result = File::open(&latest_req.path)
                    .map_err(|e| e.to_string())
                    .and_then(|file| Decoder::try_from(BufReader::new(file)).map_err(|e| e.to_string()));

                if res_tx.send((latest_req.generation, latest_req.index, result)).is_err() {
                    break;
                }
            }
        });

        // Initializes the default player instance
        let mut instance = Self {
            _handle: handle,
            player,
            queue,
            current_index: 0,
            track_start_time: Instant::now(),
            is_paused: false,
            paused_elapsed: Duration::ZERO,
            repeat_mode,
            preload_generation: 0,
            preload_tx: req_tx,
            preload_rx: res_rx,
            preloaded_track: None,
        };

        // Immediately begin preloading the initial track if available
        if !instance.queue.is_empty() {
            instance.preload_track(0);
        }

        Ok(instance)
    }

    /// Scans the queue to assign start and end index ranges for contiguous tracks of the same album.
    pub(crate) fn assign_album_ranges(queue: &mut [Track]) {
        if queue.is_empty() {
            return;
        }

        let mut start = 0;
        while start < queue.len() {
            let cur_track = &queue[start];
            let cur_artist = if cur_track.metadata.album_artist.is_empty() {
                &cur_track.metadata.artist
            } else {
                &cur_track.metadata.album_artist
            };
            let cur_album = &cur_track.metadata.album;

            let mut end = start;
            while end + 1 < queue.len() {
                let next_track = &queue[end + 1];
                let next_artist = if next_track.metadata.album_artist.is_empty() {
                    &next_track.metadata.artist
                } else {
                    &next_track.metadata.album_artist
                };

                if next_artist == cur_artist && &next_track.metadata.album == cur_album {
                    end += 1;
                } else {
                    break;
                }
            }

            for track in &mut queue[start..=end] {
                track.album_range = (start, end);
            }

            start = end + 1;
        }
    }

    /// Dispatches a non-blocking request to the worker thread to preload the track at `index`.
    fn preload_track(&mut self, index: usize) {
        let Some(path) = self.queue.get(index).map(|t| t.path.clone()) else {
            return;
        };

        self.preload_generation = self.preload_generation.wrapping_add(1);

        let _ = self.preload_tx.send(PreloadRequest {
            generation: self.preload_generation,
            index,
            path,
        });
    }

    /// Polls the worker thread response channel and captures the result if it matches the current generation.
    fn poll_preloaded(&mut self) {
        while let Ok((r#gen, index, res)) = self.preload_rx.try_recv() {
            if r#gen == self.preload_generation {
                if let Ok(source) = res {
                    self.preloaded_track = Some((index, source));
                }
            }
        }
    }

    /// Plays a track at a specific index, utilizing preloaded decoders when available.
    pub(crate) fn play_index(&mut self, index: usize, start_offset: Duration, start_paused: bool) -> Result<(), Box<dyn Error + Send + Sync>> {
        if index >= self.queue.len() {
            return Err("Index out of bounds".into());
        }

        self.current_index = index;
        self.player.stop();
        self.poll_preloaded();

        // Check if the preloaded source matches the requested track
        let source = if let Some((p_idx, source)) = self.preloaded_track.take() {
            if p_idx == index { Some(source) } else { None }
        } else {
            None
        };

        // Fallback to synchronous decode if preloading missed
        let source = match source {
            Some(src) => src,
            None => {
                let track = &self.queue[index];
                let file = File::open(&track.path)?;
                Decoder::try_from(BufReader::new(file))?
            }
        };

        self.player.append(source);

        if !start_offset.is_zero() {
            let _ = self.player.try_seek(start_offset);
        }

        self.update_timer_state(start_offset);

        if start_paused {
            self.player.pause();
            self.is_paused = true;
        } else {
            self.player.play();
            self.is_paused = false;
        }

        // Trigger preloading for the upcoming track based on active repeat settings
        let next_idx = self.next_index(false);
        self.preload_track(next_idx);

        Ok(())
    }

    /// Computes the index of the next track according to active `RepeatMode` rules.
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

    /// Returns the pre-calculated `(start, end)` bounds of the album at the given index.
    #[inline]
    fn album_bounds(&self, index: usize) -> (usize, usize) {
        self.queue.get(index).map(|track| track.album_range).unwrap_or((0, 0))
    }

    /// Skips forward to the next track manually.
    pub(crate) fn skip(&mut self) {
        if self.queue.is_empty() {
            return;
        }
        let next = self.next_index(true);
        let _ = self.play_index(next, Duration::ZERO, false);
    }

    /// Navigates to the previous track in the queue.
    pub(crate) fn previous(&mut self) -> Result<(), Box<dyn Error + Send + Sync>> {
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

    /// Automatically advances to the next track upon current track completion.
    pub(crate) fn advance_track(&mut self) {
        if self.queue.is_empty() {
            return;
        }
        let next = self.next_index(false);
        let _ = self.play_index(next, Duration::ZERO, false);
    }

    /// Toggles play/pause state while maintaining precise track time tracking.
    pub(crate) fn play_pause(&mut self) {
        if self.player.is_paused() {
            self.player.play();
            if self.is_paused {
                let now = Instant::now();
                self.track_start_time = now.checked_sub(self.paused_elapsed).unwrap_or(now);
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

    /// Cycles through `Off -> Single -> Album -> All` repeat modes.
    pub(crate) fn toggle_repeat_mode(&mut self) {
        self.repeat_mode = match self.repeat_mode {
            RepeatMode::Off => RepeatMode::Single,
            RepeatMode::Single => RepeatMode::Album,
            RepeatMode::Album => RepeatMode::All,
            RepeatMode::All => RepeatMode::Off,
        };
    }

    /// Directly jumps to and starts playing the track at `index`.
    pub(crate) fn jump_to(&mut self, index: usize) -> Result<(), Box<dyn Error + Send + Sync>> {
        self.play_index(index, Duration::ZERO, false)
    }

    /// Returns elapsed duration taking into account active pause states.
    fn get_elapsed(&self) -> Duration {
        if self.is_paused { self.paused_elapsed } else { self.track_start_time.elapsed() }
    }

    /// Seeks playback relative to the current position in seconds (positive or negative).
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

    /// Adjusts `track_start_time` or `paused_elapsed` to align with a seek target position.
    fn update_timer_state(&mut self, target_pos: Duration) {
        if self.is_paused {
            self.paused_elapsed = target_pos;
        } else {
            let now = Instant::now();
            self.track_start_time = now.checked_sub(target_pos).unwrap_or(now);
        }
    }

    /// Seeks forward the currently playing track by 10 seconds.
    pub(crate) fn seek_forward(&mut self) {
        self.seek_relative(10.0);
    }

    /// Seeks backward the currently playing track by 10 seconds.
    pub(crate) fn seek_backward(&mut self) {
        self.seek_relative(-10.0);
    }

    /// Generates styled text representing the active track's metadata. Used in the status line
    /// at the bottom of the player TUI.
    pub(crate) fn current_track_info(&self) -> StyledString {
        if let Some(track) = self.queue.get(self.current_index) {
            let meta = &track.metadata;
            let mut styled = StyledString::new();
            let bold = Style::from(Effect::Bold);
            styled.append_styled(&meta.title, bold);
            styled.append_plain(" by ");
            styled.append_styled(&meta.artist, bold);
            styled.append_plain(" on ");
            styled.append_styled(&meta.album, bold);
            styled.append_plain(" (");
            styled.append_plain(&meta.year);
            styled.append_plain(")");
            styled
        } else if self.queue.is_empty() {
            StyledString::plain("No tracks loaded")
        } else {
            StyledString::plain("Playback Finished")
        }
    }

    /// Returns the current progress as a `(current_seconds, total_seconds)` tuple to indicate track
    /// progress; used by the trck progress label in the status line.
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
