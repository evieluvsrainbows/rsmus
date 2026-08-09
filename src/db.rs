use audiotags::Tag;
use rayon::prelude::*;
use rodio::{Decoder, Source};
use rusqlite::{Connection, Result as SqlResult, params};
use std::{
    collections::HashSet,
    error::Error,
    fs::File,
    path::{Path, PathBuf},
    time::Duration,
};

use crate::player::{Track, TrackMetadata};
use crate::utils;

pub fn initialize_database(conn: &Connection) -> SqlResult<()> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS tracks (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            title TEXT NOT NULL,
            artist TEXT NOT NULL,
            album_artist TEXT NOT NULL,
            album TEXT NOT NULL,
            track_number INTEGER NOT NULL,
            year TEXT NOT NULL,
            duration INTEGER NOT NULL,
            path TEXT NOT NULL UNIQUE
        )",
        [],
    )?;
    Ok(())
}

fn collect_audio_files(dir: &Path, files: &mut Vec<PathBuf>, supported_extension: &impl Fn(&Path) -> bool) -> Result<(), std::io::Error> {
    if dir.is_dir() {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                collect_audio_files(&path, files, supported_extension)?;
            } else if path.is_file() && supported_extension(&path) {
                files.push(path);
            }
        }
    }
    Ok(())
}

pub fn scan_directory_to_db(conn: &Connection, input_dir: &str) -> Result<(), Box<dyn Error>> {
    let input_path = PathBuf::from(input_dir);
    if !input_path.is_dir() {
        return Err(format!("Specified path is not a valid directory: {}", input_dir).into());
    }

    let supported_extension = |path: &Path| -> bool {
        path.extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| {
                let lower = ext.to_lowercase();
                lower == "flac" || lower == "mp3" || lower == "m4a" || lower == "wav" || lower == "ogg"
            })
            .unwrap_or(false)
    };

    let mut entries = Vec::new();
    collect_audio_files(&input_path, &mut entries, &supported_extension)?;

    if entries.is_empty() {
        return Err(format!("No supported music files found in directory or subdirectories: {}", input_dir).into());
    }

    let extract_metadata = |filepath: &PathBuf| -> TrackMetadata {
        let fallback_name = filepath.file_name().and_then(|n| n.to_str()).unwrap_or("Unknown File").to_string();
        let mut duration = Duration::from_secs(0);
        if let Ok(file) = File::open(filepath) {
            if let Ok(source) = Decoder::try_from(file) {
                duration = source.total_duration().unwrap_or(Duration::from_secs(0));
            }
        }

        let tag_parser = Tag::default();
        if let Ok(tag) = tag_parser.read_from_path(filepath) {
            let raw_title = tag.title().unwrap_or(&fallback_name);
            TrackMetadata {
                title: utils::clean_title(raw_title),
                artist: tag.artist().unwrap_or("Unknown Artist").to_string(),
                album_artist: tag.album_artist().unwrap_or("Unknown Album Artist").to_string(),
                album: tag.album_title().unwrap_or("Unknown Album").to_string(),
                track_number: tag.track_number().unwrap_or(0),
                year: tag.year().map(|y| y.to_string()).unwrap_or_else(|| "Unknown Year".to_string()),
                duration,
            }
        } else {
            TrackMetadata {
                title: utils::clean_title(&fallback_name),
                artist: "Unknown Artist".to_string(),
                album_artist: "Unknown Album Artist".to_string(),
                album: "Unknown Album".to_string(),
                track_number: 0,
                year: "Unknown Year".to_string(),
                duration,
            }
        }
    };

    let scanned_tracks: Vec<Track> = entries
        .par_iter()
        .map(|filepath| {
            let metadata = extract_metadata(filepath);
            Track { metadata, path: filepath.clone() }
        })
        .collect();

    let track_count = scanned_tracks.len();
    let mut unique_albums = HashSet::new();
    for track in &scanned_tracks {
        unique_albums.insert(track.metadata.album.clone());
    }
    let album_count = unique_albums.len();

    let tx = conn.unchecked_transaction()?;
    {
        let mut stmt = tx.prepare(
            "INSERT OR IGNORE INTO tracks (title, artist, album_artist, album, track_number, year, duration, path)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )?;

        for track in scanned_tracks {
            stmt.execute(params![
                track.metadata.title,
                track.metadata.artist,
                track.metadata.album_artist,
                track.metadata.album,
                track.metadata.track_number,
                track.metadata.year,
                track.metadata.duration.as_secs() as i64,
                track.path.to_string_lossy().to_string()
            ])?;
        }
    }
    tx.commit()?;

    println!("Successfully processed {} tracks across {} albums for database synchronization.", track_count, album_count);
    Ok(())
}

pub fn fetch_sorted_tracks_from_db(conn: &Connection) -> SqlResult<Vec<Track>> {
    let mut stmt = conn.prepare(
        "SELECT title, artist, album_artist, album, track_number, year, duration, path
        FROM tracks
        ORDER BY artist ASC, album ASC, track_number ASC",
    )?;

    let track_iter = stmt.query_map([], |row| {
        let path_str: String = row.get(7)?;
        Ok(Track {
            metadata: TrackMetadata {
                title: row.get(0)?,
                artist: row.get(1)?,
                album_artist: row.get(2)?,
                album: row.get(3)?,
                track_number: row.get(4)?,
                year: row.get(5)?,
                duration: Duration::from_secs(row.get::<_, i64>(6)? as u64),
            },
            path: PathBuf::from(path_str),
        })
    })?;

    let mut tracks = Vec::new();
    for track in track_iter {
        tracks.push(track?);
    }

    Ok(tracks)
}
