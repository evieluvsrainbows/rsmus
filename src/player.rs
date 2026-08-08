use std::{
    error::Error,
    fs::File,
    path::PathBuf,
    time::{Duration, Instant},
};

use rodio::{Decoder, DeviceSinkBuilder, MixerDeviceSink, Player as RodioPlayer};

#[derive(Clone)]
pub struct TrackMetadata {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub year: String,
    pub duration: Duration,
}

pub struct MusicPlayer {
    _handle: MixerDeviceSink,
    pub player: RodioPlayer,
    pub tracks: Vec<TrackMetadata>,
    pub paths: Vec<PathBuf>,
    pub current_index: usize,
    pub track_start_time: Instant,
    pub is_paused: bool,
    pub paused_elapsed: Duration,
}

impl MusicPlayer {
    pub fn new(tracks: Vec<TrackMetadata>, paths: Vec<PathBuf>) -> Result<Self, Box<dyn Error>> {
        let handle = DeviceSinkBuilder::open_default_sink()?;
        let player = RodioPlayer::connect_new(&handle.mixer());
        Ok(Self {
            _handle: handle,
            player,
            tracks,
            paths,
            current_index: 0,
            track_start_time: Instant::now(),
            is_paused: false,
            paused_elapsed: Duration::ZERO,
        })
    }

    pub fn skip(&mut self) {
        if !self.player.empty() {
            self.player.skip_one();
            self.advance_track();
        }
    }

    pub fn previous(&mut self) -> Result<(), Box<dyn Error>> {
        if self.current_index > 0 {
            self.current_index -= 1;
        } else {
            self.current_index = 0;
        }

        self.player.stop();

        for filepath in self.paths.iter().skip(self.current_index) {
            let file = File::open(filepath)?;
            let source = Decoder::try_from(file)?;
            self.player.append(source);
        }

        self.track_start_time = Instant::now();
        self.paused_elapsed = Duration::ZERO;
        self.is_paused = false;
        self.player.play();

        Ok(())
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

    pub fn advance_track(&mut self) {
        self.current_index = (self.current_index + 1).min(self.tracks.len().saturating_sub(1));
        self.track_start_time = Instant::now();
        self.paused_elapsed = Duration::ZERO;
        self.is_paused = false;
    }

    pub fn current_track_info(&self) -> String {
        if self.tracks.is_empty() {
            return "No tracks loaded".to_string();
        }
        if self.current_index < self.tracks.len() {
            let t = &self.tracks[self.current_index];
            format!("{} by {} on {} ({})", t.title, t.artist, t.album, t.year)
        } else {
            "Playback Finished".to_string()
        }
    }

    pub fn get_current_progress(&self) -> (usize, usize) {
        if self.tracks.is_empty() || self.current_index >= self.tracks.len() {
            return (0, 100);
        }

        let total_dur = self.tracks[self.current_index].duration;
        if total_dur.as_secs() == 0 {
            return (0, 100);
        }

        let elapsed = if self.is_paused { self.paused_elapsed } else { self.track_start_time.elapsed() };

        let current_secs = elapsed.as_secs() as usize;
        let total_secs = total_dur.as_secs() as usize;

        (current_secs.min(total_secs), total_secs)
    }
}
