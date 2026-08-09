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

use rayon::prelude::*;
use rodio::{Decoder, DeviceSinkBuilder, MixerDeviceSink, Player, Source, source::Buffered};

type PreloadedSource = Buffered<Decoder<BufReader<File>>>;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RepeatMode {
    Off,
    One,
    Album,
    Library,
}

impl RepeatMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            RepeatMode::Off => "off",
            RepeatMode::One => "one",
            RepeatMode::Album => "album",
            RepeatMode::Library => "library",
        }
    }
}

impl FromStr for RepeatMode {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "off" => Ok(RepeatMode::Off),
            "one" => Ok(RepeatMode::One),
            "album" => Ok(RepeatMode::Album),
            "library" => Ok(RepeatMode::Library),
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
pub struct TrackMetadata {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_artist: String,
    pub year: String,
    pub track_number: u16,
    pub duration: Duration,
}

#[derive(Clone, Debug)]
pub struct Track {
    pub metadata: TrackMetadata,
    pub path: PathBuf,
}

pub struct MusicPlayer {
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
    pub fn new(queue: Vec<Track>, allowed_base_dir: Option<&Path>, repeat_mode: RepeatMode) -> Result<Self, Box<dyn Error>> {
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

    pub fn preload_track(&mut self, index: usize) {
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

    pub fn poll_preloaded(&mut self) {
        if let Some(ref rx) = self.preload_rx {
            if let Ok(result) = rx.try_recv() {
                if let Ok(data) = result {
                    self.preloaded_track = Some(data);
                }
                self.preload_rx = None;
            }
        }
    }

    pub fn play_index(&mut self, index: usize) -> Result<(), Box<dyn Error>> {
        if index >= self.queue.len() {
            return Err("Index out of bounds".into());
        }

        self.current_index = index;
        self.player.stop();
        self.poll_preloaded();

        if let Some((preloaded_index, source)) = self.preloaded_track.take() {
            if preloaded_index == index {
                self.player.append(source);
            } else if let Some(track) = self.queue.get(index) {
                let file = File::open(&track.path)?;
                let source = Decoder::try_from(BufReader::new(file))?;
                self.player.append(source);
            }
        } else if let Some(track) = self.queue.get(index) {
            let file = File::open(&track.path)?;
            let source = Decoder::try_from(BufReader::new(file))?;
            self.player.append(source);
        }

        self.track_start_time = Instant::now();
        self.paused_elapsed = Duration::ZERO;
        self.is_paused = false;
        self.player.play();

        let next_index = self.next_index();
        self.preload_track(next_index);

        Ok(())
    }

    pub fn next_index(&self) -> usize {
        if self.queue.is_empty() {
            return 0;
        }

        match self.repeat_mode {
            RepeatMode::One => self.current_index,
            RepeatMode::Album => {
                let (start, end) = self.album_bounds(self.current_index);
                if self.current_index >= end { start } else { self.current_index + 1 }
            }
            RepeatMode::Library => (self.current_index + 1) % self.queue.len(),
            RepeatMode::Off => (self.current_index + 1).min(self.queue.len() - 1),
        }
    }

    pub fn album_bounds(&self, index: usize) -> (usize, usize) {
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

    pub fn skip(&mut self) {
        if self.queue.is_empty() {
            return;
        }
        let next = self.next_index();
        let _ = self.play_index(next);
    }

    pub fn previous(&mut self) -> Result<(), Box<dyn Error>> {
        let new_index = self.current_index.saturating_sub(1);
        self.play_index(new_index)
    }

    pub fn advance_track(&mut self) {
        self.skip();
    }

    pub fn play_pause(&mut self) {
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

    pub fn toggle_repeat_mode(&mut self) {
        self.repeat_mode = match self.repeat_mode {
            RepeatMode::Off => RepeatMode::One,
            RepeatMode::One => RepeatMode::Album,
            RepeatMode::Album => RepeatMode::Library,
            RepeatMode::Library => RepeatMode::Off,
        };
    }

    pub fn jump_to(&mut self, index: usize) -> Result<(), Box<dyn Error>> {
        self.play_index(index)
    }

    pub fn current_track_info(&self) -> String {
        if let Some(track) = self.queue.get(self.current_index) {
            let t = &track.metadata;
            format!("{} by {} on {} ({})", t.title, t.artist, t.album, t.year)
        } else if self.queue.is_empty() {
            "No tracks loaded".to_string()
        } else {
            "Playback Finished".to_string()
        }
    }

    pub fn get_current_progress(&self) -> (usize, usize) {
        let Some(track) = self.queue.get(self.current_index) else {
            return (0, 100);
        };

        let total_secs = track.metadata.duration.as_secs() as usize;
        if total_secs == 0 {
            return (0, 100);
        }

        let elapsed = if self.is_paused { self.paused_elapsed } else { self.track_start_time.elapsed() };
        let current_secs = elapsed.as_secs() as usize;

        (current_secs.min(total_secs), total_secs)
    }
}
