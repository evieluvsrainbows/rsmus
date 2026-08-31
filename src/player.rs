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
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc::{Receiver, Sender, channel},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use crate::utils;

/// Type alias for the audio decoding source produced off-thread.
type PreloadedSource = Decoder<BufReader<File>>;

/// Command message sent to the background I/O worker thread to asynchronously decode a track.
struct PreloadRequest {
    /// Monotonic sequence token used to identify and discard outdated decoding requests.
    generation: u64,
    /// Queue index of the track to be preloaded.
    index: usize,
    /// File system path to the audio file.
    path: PathBuf,
}

/// Defines the queue loop and track repetition behavior.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum RepeatMode {
    #[default]
    Off,
    Single,
    Album,
    All,
}

impl RepeatMode {
    /// Returns the string key representation for serialization or state persistence.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Single => "single",
            Self::Album => "album",
            Self::All => "all",
        }
    }
}

impl FromStr for RepeatMode {
    type Err = ();

    /// Parses a string representation into a `RepeatMode`, ignoring case.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.eq_ignore_ascii_case("off") {
            Ok(Self::Off)
        } else if s.eq_ignore_ascii_case("single") {
            Ok(Self::Single)
        } else if s.eq_ignore_ascii_case("album") {
            Ok(Self::Album)
        } else if s.eq_ignore_ascii_case("all") {
            Ok(Self::All)
        } else {
            Err(())
        }
    }
}

impl fmt::Display for RepeatMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Stores metadata associated with an individual audio track.
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

impl TrackMetadata {
    /// Returns `album_artist` if non-empty, falling back to track `artist`.
    #[inline]
    pub fn effective_artist(&self) -> &str {
        if self.album_artist.is_empty() { &self.artist } else { &self.album_artist }
    }
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

/// Represents an item in the playlist queue.
#[derive(Clone, Debug)]
pub(crate) struct Track {
    pub metadata: TrackMetadata,
    pub path: PathBuf,
    /// Pre-calculated standard bounds `(start_index, end_index)` for the track's parent album.
    pub album_range: (usize, usize),
}

impl Track {
    pub fn new(metadata: TrackMetadata, path: PathBuf) -> Self {
        Self { metadata, path, album_range: (0, 0) }
    }
}

/// Core audio engine controller handling playback, track preloading, queue traversal, and state synchronization.
pub(crate) struct MusicPlayer {
    /// Audio output device sink reference kept alive to prevent sound device drops.
    _handle: MixerDeviceSink,
    /// Rodio high-level audio playback stream controller.
    pub player: Player,
    /// Playback queue containing tracks and album structure.
    pub queue: Vec<Track>,
    /// Index of the currently active track in `queue`.
    pub current_index: usize,
    /// Calculated reference timestamp for computing active track progress during playback.
    pub track_start_time: Instant,
    /// Flag designating whether playback is currently paused.
    pub is_paused: bool,
    /// Accumulated elapsed playback time prior to entering a paused state.
    pub paused_elapsed: Duration,
    /// Configured playback repetition behavior.
    pub repeat_mode: RepeatMode,
    /// Overridden or restored fixed progress state (e.g., initial state load or delayed start).
    pub saved_progress: Option<Duration>,

    /// Internal sequence token counter for invalidating stale background preloads.
    preload_generation: u64,
    /// Shared atomic counter tracking the latest active generation across threads.
    active_generation: Arc<AtomicU64>,
    /// Channel sender pushing background track preloading tasks to the worker thread.
    preload_tx: Option<Sender<PreloadRequest>>,
    /// Channel receiver consuming pre-decoded audio sources from the worker thread.
    preload_rx: Receiver<(u64, usize, Result<PreloadedSource, String>)>,
    /// Cache holding the pre-decoded source along with its corresponding queue index.
    preloaded_track: Option<(usize, PreloadedSource)>,
    /// Thread handle for graceful join operations during drop.
    worker_handle: Option<JoinHandle<()>>,
}

impl MusicPlayer {
    /// Constructs a new `MusicPlayer` instance, initializes audio devices, and starts the background preloader thread.
    pub(crate) fn new(mut queue: Vec<Track>, repeat_mode: RepeatMode, initial_progress: Option<Duration>) -> Result<Self, Box<dyn Error + Send + Sync>> {
        // Compute contiguous album bounds across the entire queue.
        Self::assign_album_ranges(&mut queue);

        // Initialize Rodio audio sink hardware device and player stream.
        let handle = DeviceSinkBuilder::open_default_sink()?;
        let player = Player::connect_new(handle.mixer());

        // Setup channel pipelines for preloader requests and incoming decoded sources.
        let (req_tx, req_rx) = channel::<PreloadRequest>();
        let (res_tx, res_rx) = channel();

        let active_generation = Arc::new(AtomicU64::new(0));
        let worker_gen = Arc::clone(&active_generation);

        // Background worker loop executing file I/O and audio decoding.
        let worker_handle = thread::spawn(move || {
            while let Ok(mut latest_req) = req_rx.recv() {
                // Drain channel backlog so only the latest preload request is processed.
                while let Ok(newer_req) = req_rx.try_recv() {
                    latest_req = newer_req;
                }

                // Check staleness before attempting expensive file I/O or audio header parsing.
                if latest_req.generation != worker_gen.load(Ordering::Relaxed) {
                    continue;
                }

                // Attempt file opening and decoder instantiation.
                let result = File::open(&latest_req.path)
                    .map_err(|e| e.to_string())
                    .and_then(|file| Decoder::try_from(BufReader::new(file)).map_err(|e| e.to_string()));

                // Re-verify staleness before sending result back to prevent populating stale cache.
                if latest_req.generation == worker_gen.load(Ordering::Relaxed) && res_tx.send((latest_req.generation, latest_req.index, result)).is_err() {
                    break; // Exit worker loop if the receiver hung up.
                }
            }
        });

        let mut instance = Self {
            _handle: handle,
            player,
            queue,
            current_index: 0,
            track_start_time: Instant::now(),
            is_paused: false,
            paused_elapsed: Duration::ZERO,
            repeat_mode,
            saved_progress: initial_progress,
            preload_generation: 0,
            active_generation,
            preload_tx: Some(req_tx),
            preload_rx: res_rx,
            preloaded_track: None,
            worker_handle: Some(worker_handle),
        };

        // Instantly trigger preloading for the initial track if the queue is non-empty.
        if !instance.queue.is_empty() {
            instance.preload_track(0);
        }

        Ok(instance)
    }

    /// Scans the queue to assign contiguous album index boundaries `(start, end)` to all tracks.
    pub(crate) fn assign_album_ranges(queue: &mut [Track]) {
        if queue.is_empty() {
            return;
        }

        let mut start = 0;
        while start < queue.len() {
            let cur_artist = queue[start].metadata.effective_artist();
            let cur_album = &queue[start].metadata.album;

            let mut end = start;
            while end + 1 < queue.len() {
                let next = &queue[end + 1];
                if next.metadata.effective_artist() == cur_artist && &next.metadata.album == cur_album {
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

    /// Dispatches an asynchronous preload request for the track at `index`.
    fn preload_track(&mut self, index: usize) {
        let Some(path) = self.queue.get(index).map(|t| t.path.clone()) else {
            return;
        };

        // Clear existing cached track data.
        self.preloaded_track = None;

        // Bump local sequence counter and update atomic flag to cancel pending worker jobs.
        self.preload_generation = self.preload_generation.wrapping_add(1);
        self.active_generation.store(self.preload_generation, Ordering::Relaxed);

        if let Some(tx) = &self.preload_tx {
            let _ = tx.send(PreloadRequest {
                generation: self.preload_generation,
                index,
                path,
            });
        }
    }

    /// Polls the result channel and caches completed audio decodes matching the current generation token.
    fn poll_preloaded(&mut self) {
        while let Ok((preload_req, index, res)) = self.preload_rx.try_recv() {
            if preload_req == self.preload_generation
                && let Ok(source) = res
            {
                self.preloaded_track = Some((index, source));
            }
        }
    }

    /// Starts playback of the track at `index`, optionally seeking to an offset or starting paused.
    pub(crate) fn play_index(&mut self, index: usize, start_offset: Duration, start_paused: bool) -> Result<(), Box<dyn Error + Send + Sync>> {
        if index >= self.queue.len() {
            return Err("Index out of bounds".into());
        }

        // Clear saved progress unless resuming the exact same track paused.
        if index != self.current_index || !start_paused {
            self.saved_progress = None;
        }

        self.current_index = index;
        self.player.stop();
        self.poll_preloaded();

        // Use pre-decoded source if available; fall back to synchronous disk load on cache miss.
        let preloaded = self.preloaded_track.take().filter(|(p_idx, _)| *p_idx == index).map(|(_, src)| src);
        let source = match preloaded {
            Some(src) => src,
            None => {
                let track = &self.queue[index];
                let file = File::open(&track.path)?;
                Decoder::try_from(BufReader::new(file))?
            }
        };

        self.player.append(source);

        // Apply starting position offset if specified.
        if !start_offset.is_zero() {
            let _ = self.player.try_seek(start_offset);
        }

        // Set initial playback state.
        if start_paused {
            self.player.pause();
            self.is_paused = true;
        } else {
            self.player.play();
            self.is_paused = false;
        }

        self.update_timer_state(start_offset);

        // Pre-fetch the anticipated next track asynchronously.
        let next_idx = self.next_index(true);
        self.preload_track(next_idx);

        Ok(())
    }

    /// Calculates the next track index according to the configured `RepeatMode`.
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

    /// Helper retrieving pre-computed album index boundaries for a track.
    #[inline]
    fn album_bounds(&self, index: usize) -> (usize, usize) {
        self.queue.get(index).map(|track| track.album_range).unwrap_or((0, 0))
    }

    /// Skips to the next track in queue.
    pub(crate) fn skip(&mut self) {
        if self.queue.is_empty() {
            return;
        }
        let next = self.next_index(true);
        let _ = self.play_index(next, Duration::ZERO, false);
    }

    /// Navigates to the previous track in queue based on repeat rules.
    pub(crate) fn previous(&mut self) -> Result<(), Box<dyn Error + Send + Sync>> {
        if self.queue.is_empty() {
            return Ok(());
        }

        let new_index = match self.repeat_mode {
            RepeatMode::Single | RepeatMode::All => {
                if self.current_index == 0 {
                    self.queue.len() - 1
                } else {
                    self.current_index - 1
                }
            }
            _ => {
                if self.current_index == 0 {
                    0
                } else {
                    self.current_index - 1
                }
            }
        };

        self.play_index(new_index, Duration::ZERO, false)
    }

    /// Automatically advances to the next track upon stream depletion.
    pub(crate) fn advance_track(&mut self) {
        if self.queue.is_empty() {
            return;
        }
        let next = self.next_index(false);
        let _ = self.play_index(next, Duration::ZERO, false);
    }

    /// Toggles between active playback and paused states, maintaining elapsed progress accuracy.
    pub(crate) fn play_pause(&mut self) {
        if self.player.is_paused() {
            self.player.play();
            if self.is_paused {
                let now = Instant::now();
                self.track_start_time = now.checked_sub(self.paused_elapsed).unwrap_or(now);
                self.is_paused = false;
                self.saved_progress = None;
            }
        } else {
            self.player.pause();
            if !self.is_paused {
                self.paused_elapsed = self.track_start_time.elapsed();
                self.is_paused = true;
            }
        }
    }

    /// Cycles through `RepeatMode` variants.
    pub(crate) fn toggle_repeat_mode(&mut self) {
        self.repeat_mode = match self.repeat_mode {
            RepeatMode::Off => RepeatMode::Single,
            RepeatMode::Single => RepeatMode::Album,
            RepeatMode::Album => RepeatMode::All,
            RepeatMode::All => RepeatMode::Off,
        };
    }

    /// Direct jump to an arbitrary index in the queue.
    pub(crate) fn jump_to(&mut self, index: usize) -> Result<(), Box<dyn Error + Send + Sync>> {
        self.play_index(index, Duration::ZERO, false)
    }

    /// Returns the currently accumulated elapsed time for the active track.
    fn get_elapsed(&self) -> Duration {
        if self.is_paused { self.paused_elapsed } else { self.track_start_time.elapsed() }
    }

    /// Relative seek forward/backward by `seconds`. Advances track if seeking past total length.
    fn seek_relative(&mut self, seconds: f64) {
        let Some(track) = self.queue.get(self.current_index) else {
            return;
        };

        let current_pos = self.get_elapsed();
        let total_duration = track.metadata.duration;

        let target_pos = if seconds >= 0.0 {
            current_pos + Duration::from_secs_f64(seconds)
        } else {
            current_pos.saturating_sub(Duration::from_secs_f64(seconds.abs()))
        };

        // Advance track automatically if seeking within 500ms of end.
        let safety_boundary = total_duration.saturating_sub(Duration::from_millis(500));
        if seconds >= 0.0 && target_pos >= safety_boundary {
            self.advance_track();
            return;
        }

        if self.player.try_seek(target_pos).is_ok() {
            self.update_timer_state(target_pos);
        } else {
            // Re-open source fallback if underlying seek operation fails.
            let _ = self.play_index(self.current_index, target_pos, self.is_paused);
        }
    }

    /// Re-calculates tracking timers after seeking.
    fn update_timer_state(&mut self, target_pos: Duration) {
        if self.is_paused {
            self.paused_elapsed = target_pos;
        } else {
            let now = Instant::now();
            self.track_start_time = now.checked_sub(target_pos).unwrap_or(now);
        }
    }

    pub(crate) fn seek_forward(&mut self) {
        self.seek_relative(10.0);
    }

    pub(crate) fn seek_backward(&mut self) {
        self.seek_relative(-10.0);
    }

    /// Formats metadata of the current track into a single Cursive `StyledString` with minimal allocations.
    pub(crate) fn current_track_info(&self) -> StyledString {
        let Some(track) = self.queue.get(self.current_index) else {
            return StyledString::plain(if self.queue.is_empty() { "No tracks loaded" } else { "Playback Finished" });
        };

        let meta = &track.metadata;
        let bold = Style::from(Effect::Bold);

        let mut styled = StyledString::new();
        styled.append_styled(&meta.title, bold);
        styled.append_plain(" by ");
        styled.append_styled(&meta.artist, bold);
        styled.append_plain(" on ");
        styled.append_styled(&meta.album, bold);
        styled.append_plain(format!(" ({})", meta.year));
        styled
    }

    /// Calculates current track progress returned as standard `(elapsed_seconds, total_seconds)` tuple.
    pub(crate) fn get_current_progress(&self) -> (usize, usize) {
        let Some(track) = self.queue.get(self.current_index) else {
            return (0, 100);
        };

        let total_secs = track.metadata.duration.as_secs() as usize;
        if total_secs == 0 {
            return (0, 100);
        }

        let elapsed = self.saved_progress.unwrap_or_else(|| self.get_elapsed());
        let current_secs = elapsed.min(track.metadata.duration).as_secs() as usize;

        (current_secs.min(total_secs), total_secs)
    }
}

/// Custom `Drop` implementation to safely tear down the background channel and join the worker thread.
impl Drop for MusicPlayer {
    fn drop(&mut self) {
        // Drop channel sender to break worker thread receive loop.
        self.preload_tx.take();
        if let Some(handle) = self.worker_handle.take() {
            let _ = handle.join();
        }
    }
}
