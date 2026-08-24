use crate::player::{RepeatMode, Track, TrackMetadata};
use crate::utils;
use audiotags::Tag;
use rayon::prelude::*;
use rusqlite::{Connection, OptionalExtension, Result, params};
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

/// Scans a given directory or directories into the SQLite database.
pub(crate) fn scan_directories_to_db<I, P>(conn: &mut Connection, dirs: I) -> Result<(), Box<dyn Error>>
where
    I: IntoIterator<Item = P>,
    P: AsRef<Path>,
{
    let mut valid_dirs_count = 0;
    let mut entries = Vec::new();

    for dir in dirs {
        let input_path = dir.as_ref();
        if !input_path.is_dir() {
            eprintln!("Warning: Skipping invalid directory path: {}", input_path.display());
            continue;
        }
        collect_audio_files(input_path, &mut entries)?;
        valid_dirs_count += 1;
    }

    if valid_dirs_count == 0 {
        return Err("No valid input directories were provided.".into());
    }

    if entries.is_empty() {
        return Err("No supported music files found in any of the specified directories.".into());
    }

    // prior to feeding the entries vec into the scanned_tracks array, ensure that
    // the entries array has been sorted and deduplicated. this will avoid cases where
    // if a user scans both a directory and a subdirectory within the same directory,
    // track_count won't show double the tracks that have actually been added to the
    // database.
    entries.sort();
    entries.dedup();

    let scanned_tracks: Vec<Track> = entries
        .into_par_iter()
        .map(|path| {
            let metadata = extract_track_metadata(&path);
            Track { metadata, path }
        })
        .collect();

    let album_count = scanned_tracks.iter().map(|t| &t.metadata.album).collect::<HashSet<_>>().len();
    let track_count = scanned_tracks.len();
    let mut updated_path_count = 0;

    let tx = conn.transaction()?;
    {
        let current_track_id: Option<i64> = tx
            .query_row("SELECT value FROM settings WHERE key = 'last_track_index'", [], |row| {
                let val_str: String = row.get(0)?;
                Ok(val_str.parse::<i64>().unwrap_or(-1))
            })
            .optional()?;

        let mut select_existing_stmt = tx.prepare("SELECT id, path FROM tracks WHERE title = ?1 AND artist = ?2 LIMIT 1")?;
        let mut update_stmt = tx.prepare(
            "UPDATE tracks
             SET album_artist = ?1, album = ?2, track_number = ?3, year = ?4, duration = ?5, path = ?6
             WHERE id = ?7",
        )?;

        let mut insert_stmt = tx.prepare(
            "INSERT OR IGNORE INTO tracks (title, artist, album_artist, album, track_number, year, duration, path)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )?;

        for track in &scanned_tracks {
            let path_str = track.path.to_str().ok_or("Path contains invalid UTF-8")?;
            let existing_record: Option<(i64, String)> = select_existing_stmt
                .query_row(params![track.metadata.title, track.metadata.artist], |row| Ok((row.get(0)?, row.get(1)?)))
                .optional()?;

            if let Some((id, existing_path)) = existing_record {
                update_stmt.execute(params![
                    track.metadata.album_artist,
                    track.metadata.album,
                    track.metadata.track_number,
                    track.metadata.year,
                    track.metadata.duration.as_secs() as i64,
                    path_str,
                    id
                ])?;

                if existing_path != path_str {
                    updated_path_count += 1;
                }
            } else {
                insert_stmt.execute(params![
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

        let mut select_all_stmt = tx.prepare("SELECT id, path FROM tracks")?;
        let existing_tracks = select_all_stmt.query_map([], |row| {
            let id: i64 = row.get(0)?;
            let path: String = row.get(1)?;
            Ok((id, path))
        })?;

        let mut active_track_removed = false;
        let mut delete_stmt = tx.prepare("DELETE FROM tracks WHERE id = ?1")?;
        for track_res in existing_tracks {
            let (id, path_str) = track_res?;
            let path = Path::new(&path_str);
            if !path.exists() {
                delete_stmt.execute(params![id])?;
                println!("Deleted track with path {} from database as it no longer exists.", path.display());
                if Some(id) == current_track_id {
                    active_track_removed = true;
                }
            }
        }

        if active_track_removed {
            tx.execute("INSERT OR REPLACE INTO settings (key, value) VALUES ('last_track_index', '0')", [])?;
            tx.execute("INSERT OR REPLACE INTO settings (key, value) VALUES ('last_track_progress', '0')", [])?;
        }
    }
    tx.commit()?;

    let track_label = if track_count == 1 { "track" } else { "tracks" };
    let album_label = if album_count == 1 { "album" } else { "albums" };
    let dir_label = if valid_dirs_count == 1 { "directory" } else { "directories" };
    if updated_path_count > 0 {
        println!("Updated paths for {updated_path_count} existing {track_label}.");
    }
    println!("Successfully synced {track_count} {track_label} across {album_count} {album_label} and {valid_dirs_count} {dir_label} to the database.");
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
