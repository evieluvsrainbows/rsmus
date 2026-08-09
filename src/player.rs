use std::{
    error::Error,
    fs::File,
    io::BufReader,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use rodio::{Decoder, DeviceSinkBuilder, MixerDeviceSink, Player as RodioPlayer};

#[derive(Clone)]
pub struct TrackMetadata {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_artist: String,
    pub year: String,
    pub track_number: u16,
    pub duration: Duration,
}

#[derive(Clone)]
pub struct Track {
    pub metadata: TrackMetadata,
    pub path: PathBuf,
}

pub struct MusicPlayer {
    _handle: MixerDeviceSink,
    pub player: RodioPlayer,
    pub queue: Vec<Track>,
    pub current_index: usize,
    pub track_start_time: Instant,
    pub is_paused: bool,
    pub paused_elapsed: Duration,
}

impl MusicPlayer {
    pub fn new(queue: Vec<Track>, allowed_base_dir: Option<&Path>) -> Result<Self, Box<dyn Error>> {
        if let Some(base) = allowed_base_dir {
            let canonical_base = base.canonicalize()?;
            for track in &queue {
                let canonical_path = track.path.canonicalize()?;
                if !canonical_path.starts_with(&canonical_base) {
                    return Err(format!("Security error: Path traversal detected: {:?}", track.path).into());
                }
            }
        }

        let handle = DeviceSinkBuilder::open_default_sink()?;
        let player = RodioPlayer::connect_new(&handle.mixer());

        Ok(Self {
            _handle: handle,
            player,
            queue,
            current_index: 0,
            track_start_time: Instant::now(),
            is_paused: false,
            paused_elapsed: Duration::ZERO,
        })
    }

    pub fn play_index(&mut self, index: usize) -> Result<(), Box<dyn Error>> {
        self.current_index = index;
        self.player.stop();

        if let Some(track) = self.queue.get(self.current_index) {
            let file = File::open(&track.path)?;
            let source = Decoder::try_from(BufReader::new(file))?;
            self.player.append(source);
        }

        self.track_start_time = Instant::now();
        self.paused_elapsed = Duration::ZERO;
        self.is_paused = false;
        self.player.play();

        Ok(())
    }

    pub fn skip(&mut self) {
        let max_index = self.queue.len().saturating_sub(1);
        if self.current_index < max_index {
            let _ = self.play_index(self.current_index + 1);
        }
    }

    pub fn previous(&mut self) -> Result<(), Box<dyn Error>> {
        let new_index = self.current_index.saturating_sub(1);
        self.play_index(new_index)
    }

    pub fn advance_track(&mut self) {
        let max_index = self.queue.len().saturating_sub(1);
        if self.current_index < max_index {
            let _ = self.play_index(self.current_index + 1);
        }
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

    pub fn jump_to(&mut self, index: usize) -> Result<(), Box<dyn std::error::Error>> {
        if index >= self.queue.len() {
            return Err("Track index out of bounds".into());
        }

        self.player.stop();
        self.current_index = index;

        for i in self.current_index..self.queue.len() {
            let file = File::open(&self.queue[i].path)?;
            let source = Decoder::try_from(BufReader::new(file))?;
            self.player.append(source);
        }

        self.track_start_time = Instant::now();
        self.is_paused = false;
        self.player.play();

        Ok(())
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
