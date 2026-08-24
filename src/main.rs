mod db;
mod player;
mod ui;
mod utils;

use clap::Parser;
use rusqlite::Connection;
use std::{
    collections::BTreeMap,
    error::Error,
    sync::{Arc, Mutex},
};

type TrackHierarchy = BTreeMap<String, BTreeMap<String, Vec<(usize, player::Track)>>>;
type SharedState<T> = Arc<Mutex<T>>;

#[derive(Parser, Debug)]
#[clap(about, version)]
struct Args {
    #[arg(short, long, value_name = "FOLDER", num_args = 1..)]
    /// Scans a directory or directories to the database.
    scan: Option<Vec<String>>,
}

fn main() -> Result<(), Box<dyn Error>> {
    let args = Args::parse();
    let mut conn = Connection::open("rsmus.db")?;
    db::initialize_database(&conn)?;

    if let Some(folder_paths) = args.scan {
        db::scan_directories_to_db(&mut conn, &folder_paths)?;
        return Ok(());
    }

    let playlist = db::fetch_sorted_tracks_from_db(&conn)?;
    if playlist.is_empty() {
        return Err("No tracks found in the database. Run 'rsmus --scan <folder>' first to import music.".into());
    }

    let repeat_mode = db::load_repeat_mode(&conn);
    let (last_index, last_progress) = db::load_last_played_state(&conn);
    let music_player = utils::initialize_player(playlist, repeat_mode, last_index, last_progress)?;
    let hierarchy = ui::library::build_hierarchy(&music_player)?;
    let (expanded_artists, expanded_albums) = ui::library::get_initial_expanded_states(&hierarchy);
    let mut siv = cursive::default();

    ui::setup_cursive_theme_and_menu(&mut siv, music_player.clone());
    ui::setup_ui_layout(&mut siv, &hierarchy, music_player.clone(), expanded_artists.clone(), expanded_albums.clone())?;
    ui::handle_repeat_mode(&mut siv, music_player.clone());
    ui::register_callbacks(&mut siv, music_player.clone(), &hierarchy, expanded_artists.clone(), expanded_albums.clone());
    ui::spawn_playback_thread(siv.cb_sink().clone(), music_player.clone(), hierarchy, expanded_artists, expanded_albums);

    siv.run();

    Ok(())
}
