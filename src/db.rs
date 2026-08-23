use crate::player::{RepeatMode, Track, TrackMetadata};
use crate::utils;
use audiotags::Tag;
use rayon::prelude::*;
use rusqlite::{Connection, Result, params};
use std::{
    collections::HashSet,
    error::Error,
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

pub(crate) fn initialize_database(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS tracks(
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            title TEXT NOT NULL,
            artist TEXT NOT NULL,
            album_artist TEXT NOT NULL,
            album TEXT NOT NULL,
            track_number INTEGER NOT NULL,
            year TEXT NOT NULL,
            duration INTEGER NOT NULL,
            path TEXT NOT NULL UNIQUE
        );

        CREATE TABLE IF NOT EXISTS settings(
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_tracks_sorting ON tracks(
            COALESCE(NULLIF(album_artist, ''), artist), album, track_number, title
        );",
    )?;

    Ok(())
}

#[inline]
fn is_supported_audio_file(path: &Path) -> bool {
    let ext = match path.extension().and_then(OsStr::to_str) {
        Some(e) => e,
        None => return false,
    };

    ext.eq_ignore_ascii_case("flac") || ext.eq_ignore_ascii_case("mp3") || ext.eq_ignore_ascii_case("m4a") || ext.eq_ignore_ascii_case("wav") || ext.eq_ignore_ascii_case("ogg")
}

fn collect_audio_files(dir: &Path, files: &mut Vec<PathBuf>) -> Result<(), std::io::Error> {
    if dir.is_dir() {
        for entry in fs::read_dir(dir)? {
            let path = entry?.path();
            if path.is_dir() {
                collect_audio_files(&path, files)?;
            } else if path.is_file() && is_supported_audio_file(&path) {
                files.push(path);
            }
        }
    }
    Ok(())
}

fn extract_track_metadata(filepath: &Path) -> TrackMetadata {
    let fallback_name = filepath.file_name().and_then(|n| n.to_str()).unwrap_or("Unknown File");
    let tag = Tag::default().read_from_path(filepath).ok();
    let duration_secs = tag.as_ref().and_then(|t| t.duration()).map(|d| d as u64).unwrap_or(0);

    TrackMetadata {
        title: utils::clean_title(tag.as_ref().and_then(|t| t.title()).unwrap_or(fallback_name)),
        artist: tag.as_ref().and_then(|t| t.artist()).unwrap_or("Unknown Artist").to_string(),
        album_artist: tag.as_ref().and_then(|t| t.album_artist()).unwrap_or("Unknown Album Artist").to_string(),
        album: tag.as_ref().and_then(|t| t.album_title()).unwrap_or("Unknown Album").to_string(),
        track_number: tag.as_ref().and_then(|t| t.track_number()).unwrap_or(0),
        year: tag.as_ref().and_then(|t| t.year()).map(|y| y.to_string()).unwrap_or_else(|| "Unknown Year".to_string()),
        duration: Duration::from_secs(duration_secs),
    }
}

pub(crate) fn scan_directory_to_db(conn: &mut Connection, input_dir: impl AsRef<Path>) -> Result<(), Box<dyn Error>> {
    let input_path = input_dir.as_ref();
    if !input_path.is_dir() {
        return Err(format!("Specified path is not a valid directory: {}", input_path.display()).into());
    }

    let mut entries = Vec::new();
    collect_audio_files(input_path, &mut entries)?;

    if entries.is_empty() {
        return Err(format!("No supported music files found in directory: {}", input_path.display()).into());
    }

    let scanned_tracks: Vec<Track> = entries
        .into_par_iter()
        .map(|path| {
            let metadata = extract_track_metadata(&path);
            Track { metadata, path }
        })
        .collect();

    let album_count = scanned_tracks.iter().map(|t| &t.metadata.album).collect::<HashSet<_>>().len();
    let track_count = scanned_tracks.len();

    let tx = conn.transaction()?;
    {
        let mut stmt = tx.prepare(
            "INSERT OR IGNORE INTO tracks (title, artist, album_artist, album, track_number, year, duration, path)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )?;

        for track in &scanned_tracks {
            let path_str = track.path.to_str().ok_or("Path contains invalid UTF-8")?;

            stmt.execute(params![
                track.metadata.title,
                track.metadata.artist,
                track.metadata.album_artist,
                track.metadata.album,
                track.metadata.track_number,
                track.metadata.year,
                track.metadata.duration.as_secs() as i64,
                path_str
            ])?;
        }
    }
    tx.commit()?;

    println!("Successfully processed {} tracks across {} albums for database synchronization.", track_count, album_count);
    Ok(())
}

/// Retrieves the current repeat mode from the database.
pub(crate) fn load_repeat_mode(conn: &Connection) -> RepeatMode {
    match conn.query_row("SELECT value FROM settings WHERE key = 'repeat_mode'", [], |row| row.get::<_, String>(0)) {
        Ok(val) => match val.as_str() {
            "Single" => RepeatMode::Single,
            "Album" => RepeatMode::Album,
            "All" => RepeatMode::All,
            _ => RepeatMode::Off,
        },
        Err(_) => RepeatMode::Off,
    }
}

pub(crate) fn save_last_played_state(conn: &Connection, track_index: usize, progress_secs: usize) -> Result<()> {
    conn.execute("INSERT OR REPLACE INTO settings (key, value) VALUES ('last_track_index', ?1)", [track_index.to_string()])?;
    conn.execute("INSERT OR REPLACE INTO settings (key, value) VALUES ('last_track_progress', ?1)", [progress_secs.to_string()])?;
    Ok(())
}

pub(crate) fn load_last_played_state(conn: &Connection) -> (usize, usize) {
    let index: usize = conn
        .query_row("SELECT value FROM settings WHERE key = 'last_track_index'", [], |row| row.get::<_, String>(0))
        .ok()
        .and_then(|val| val.parse().ok())
        .unwrap_or(0);

    let progress: usize = conn
        .query_row("SELECT value FROM settings WHERE key = 'last_track_progress'", [], |row| row.get::<_, String>(0))
        .ok()
        .and_then(|val| val.parse().ok())
        .unwrap_or(0);

    (index, progress)
}

pub(crate) fn fetch_sorted_tracks_from_db(conn: &Connection) -> Result<Vec<Track>> {
    let mut stmt = conn.prepare(
        "SELECT title, artist, album, album_artist, year, track_number, duration, path FROM tracks
         ORDER BY COALESCE(NULLIF(album_artist, ''), artist) ASC, album ASC, track_number ASC, title ASC",
    )?;

    stmt.query_map([], |row| {
        let path_str: String = row.get(7)?;
        Ok(Track {
            metadata: TrackMetadata {
                title: row.get(0)?,
                artist: row.get(1)?,
                album: row.get(2)?,
                album_artist: row.get(3)?,
                year: row.get(4)?,
                track_number: row.get(5)?,
                duration: Duration::from_secs(row.get::<_, i64>(6)? as u64),
            },
            path: PathBuf::from(path_str),
        })
    })?
    .collect()
}
