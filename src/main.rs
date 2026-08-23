mod db;
mod player;
mod ui;
mod ui_utils;
mod utils;

use crate::player::{MusicPlayer, RepeatMode};
use clap::Parser;
use rodio::Decoder;
use rusqlite::Connection;
use std::{
    collections::BTreeMap,
    error::Error,
    fs::File,
    sync::{Arc, Mutex},
    time::Instant,
};

type TrackHierarchy = BTreeMap<String, BTreeMap<String, Vec<(usize, player::Track)>>>;
type SharedState<T> = Arc<Mutex<T>>;

#[derive(Parser, Debug)]
#[clap(about, version)]
struct Args {
    #[arg(short, long, value_name = "FOLDER")]
    scan: Option<String>,
}

fn initialize_player(playlist: Vec<player::Track>, repeat_mode: RepeatMode) -> Result<SharedState<MusicPlayer>, Box<dyn Error>> {
    let music_player = Arc::new(Mutex::new(player::MusicPlayer::new(playlist, None, repeat_mode)?));
    {
        let mut mp = music_player.lock().map_err(|_| "Mutex poisoned")?;
        for track in &mp.queue {
            let file = File::open(&track.path)?;
            let source = Decoder::try_from(std::io::BufReader::new(file))?;
            mp.player.append(source);
        }
        mp.track_start_time = Instant::now();

        if let Some(track) = mp.queue.get(mp.current_index) {
            let t = &track.metadata;
            utils::update_terminal_title(&t.title, &t.artist, &t.album, &t.year, mp.is_paused);
        }
    }
    Ok(music_player)
}

fn main() -> Result<(), Box<dyn Error>> {
    let args = Args::parse();
    let mut conn = Connection::open("rsmus.db")?;
    db::initialize_database(&conn)?;

    if let Some(folder_path) = args.scan {
        db::scan_directory_to_db(&mut conn, &folder_path)?;
        return Ok(());
    }

    let playlist = db::fetch_sorted_tracks_from_db(&conn)?;
    if playlist.is_empty() {
        return Err("No tracks found in the database. Run 'rsmus --scan <folder>' first to import music.".into());
    }

    let repeat_mode = db::load_repeat_mode(&conn);
    let music_player = initialize_player(playlist, repeat_mode)?;
    let hierarchy = ui::build_hierarchy(&music_player)?;
    let (expanded_artists, expanded_albums) = ui::get_initial_expanded_states(&hierarchy);
    let mut siv = cursive::default();

    ui::setup_cursive_theme_and_menu(&mut siv);
    ui::setup_ui_layout(&mut siv, &hierarchy, Arc::clone(&music_player), Arc::clone(&expanded_artists), Arc::clone(&expanded_albums))?;
    ui::handle_repeat_mode(&mut siv, Arc::clone(&music_player));
    ui::register_callbacks(&mut siv, Arc::clone(&music_player), &hierarchy, Arc::clone(&expanded_artists), Arc::clone(&expanded_albums));
    ui::spawn_playback_thread(siv.cb_sink().clone(), Arc::clone(&music_player), hierarchy, expanded_artists, expanded_albums);

    siv.run();

    Ok(())
}
