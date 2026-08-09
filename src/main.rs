mod db;
mod player;
mod ui;
mod utils;

use clap::Parser;
use cursive::{
    event::{self, Event, Key},
    menu,
    theme::Theme,
    view::{Resizable, Scrollable},
    views::{Dialog, DummyView, LinearLayout, NamedView, Panel, ScrollView, SelectView, TextView},
};
use rodio::Decoder;
use rusqlite::Connection;
use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fs::File,
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use crate::{player::RepeatMode, ui::TreeItemKey};

#[derive(Parser, Debug)]
#[clap(about, version)]
struct Args {
    #[arg(short, long, value_name = "FOLDER")]
    scan: Option<String>,
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

    let repeat_mode = match conn.query_row("SELECT value FROM settings WHERE key = 'repeat_mode'", [], |row| row.get::<_, String>(0)) {
        Ok(val) => match val.as_str() {
            "One" => RepeatMode::One,
            "Album" => RepeatMode::Album,
            "Library" => RepeatMode::Library,
            _ => RepeatMode::Off,
        },
        Err(_) => RepeatMode::Off,
    };

    let mut siv = cursive::default();
    siv.menubar()
        .add_subtree("File", menu::Tree::new().leaf("Quit", |s| s.quit()))
        .add_subtree("Help", menu::Tree::new().leaf("About", |s| s.add_layer(Dialog::info("rsmus 0.0.1").title("About rsmus"))));
    siv.set_autohide_menu(false);
    siv.set_theme(Theme::terminal_default());

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

    let initial_info = music_player.lock().map_err(|_| "Poisoned mutex")?.current_track_info();
    let initial_duration = music_player
        .lock()
        .map_err(|_| "Poisoned mutex")?
        .queue
        .get(0)
        .map(|t| t.metadata.duration.as_secs() as usize)
        .unwrap_or(100);

    let initial_time_text = format!("0:00 / {}", utils::format_time(initial_duration));
    let indicator_label = NamedView::new("play_indicator", TextView::new("▶ "));
    let track_label = NamedView::new("track_info", TextView::new(format!("{}", initial_info)).no_wrap()).max_height(1);
    let time_label = NamedView::new("time_label", TextView::new(initial_time_text));
    let repeat_label = NamedView::new("repeat_label", TextView::new("[Repeat: Off]"));

    let status_bar = LinearLayout::horizontal()
        .child(indicator_label)
        // Limit height to 1 line and allow it to take available remaining width
        .child(track_label.max_height(1).full_width())
        .child(DummyView)
        .child(repeat_label)
        .child(TextView::new("  "))
        .child(time_label);

    let mut hierarchy: BTreeMap<String, BTreeMap<String, Vec<(usize, player::Track)>>> = BTreeMap::new();
    {
        let mp = music_player.lock().map_err(|_| "Mutex poisoned")?;
        for (i, track) in mp.queue.iter().enumerate() {
            let album_artist = if track.metadata.album_artist.is_empty() {
                track.metadata.artist.clone()
            } else {
                track.metadata.album_artist.clone()
            };
            let album = track.metadata.album.clone();
            hierarchy.entry(album_artist).or_default().entry(album).or_default().push((i, track.clone()));
        }
    }

    // Ensure tracks within each album are sorted by track_number
    for albums in hierarchy.values_mut() {
        for tracks in albums.values_mut() {
            tracks.sort_by_key(|(_, track)| track.metadata.track_number);
        }
    }

    let mut initial_artists = BTreeSet::new();
    let mut initial_albums = BTreeSet::new();
    for (artist, albums) in &hierarchy {
        initial_artists.insert(artist.clone());
        for album in albums.keys() {
            initial_albums.insert((artist.clone(), album.clone()));
        }
    }

    let expanded_artists = Arc::new(Mutex::new(initial_artists));
    let expanded_albums = Arc::new(Mutex::new(initial_albums));

    let mut select_view = SelectView::<ui::TreeItemKey>::new();
    let hierarchy_for_submit = hierarchy.clone();
    let player_for_submit = Arc::clone(&music_player);

    select_view.set_on_submit(move |s, item_key| {
        let target_idx = match item_key {
            TreeItemKey::Artist(_) => return,
            TreeItemKey::Track(idx) => *idx,
            TreeItemKey::Album(artist, album) => {
                let Some((idx, _)) = hierarchy_for_submit.get(artist).and_then(|a| a.get(album)).and_then(|t| t.first()) else {
                    return;
                };
                *idx
            }
        };

        if let Ok(mut mp) = player_for_submit.lock() {
            let _ = mp.jump_to(target_idx);
        }

        if matches!(item_key, TreeItemKey::Album(..)) {
            if let Some(mut scroll_view) = s.find_name::<ScrollView<SelectView<TreeItemKey>>>("library_view") {
                let sv = scroll_view.get_inner_mut();
                let target_key = TreeItemKey::Track(target_idx);

                if let Some(pos) = (0..sv.len()).position(|i| sv.get_item(i).map_or(false, |(_, k)| *k == target_key)) {
                    sv.set_selection(pos);
                }
            }
        }
    });

    ui::rebuild_library_view(&mut select_view, &hierarchy, &expanded_artists.lock().unwrap(), &expanded_albums.lock().unwrap());

    let library_panel = Panel::new(NamedView::new("library_view", select_view.scrollable())).title("Music Library");
    let root_layout = LinearLayout::vertical().child(library_panel.full_height()).child(status_bar).full_screen();

    siv.add_layer(root_layout);

    let artists_for_space = Arc::clone(&expanded_artists);
    let albums_for_space = Arc::clone(&expanded_albums);
    let hierarchy_for_space = hierarchy.clone();

    siv.add_global_callback(' ', move |s| {
        if let Some(mut scroll_view) = s.find_name::<ScrollView<SelectView<TreeItemKey>>>("library_view") {
            let sv = scroll_view.get_inner_mut();
            if let Some(item_key) = sv.selection().map(|v| (*v).clone()) {
                match item_key {
                    TreeItemKey::Artist(artist_name) => {
                        let mut artists = artists_for_space.lock().unwrap();
                        if artists.contains(&artist_name) {
                            artists.remove(&artist_name);
                        } else {
                            artists.insert(artist_name.clone());
                        }
                        let artists_snapshot = artists.clone();
                        let albums_snapshot = albums_for_space.lock().unwrap().clone();
                        drop(artists);
                        ui::rebuild_library_view(sv, &hierarchy_for_space, &artists_snapshot, &albums_snapshot);
                    }
                    TreeItemKey::Album(artist_name, album_name) => {
                        let album_key = (artist_name, album_name);
                        let mut albums = albums_for_space.lock().unwrap();
                        if albums.contains(&album_key) {
                            albums.remove(&album_key);
                        } else {
                            albums.insert(album_key);
                        }
                        let artists_snapshot = artists_for_space.lock().unwrap().clone();
                        let albums_snapshot = albums.clone();
                        drop(albums);
                        ui::rebuild_library_view(sv, &hierarchy_for_space, &artists_snapshot, &albums_snapshot);
                    }
                    TreeItemKey::Track(_) => {}
                }
            }
        }
    });

    let player_for_thread = Arc::clone(&music_player);
    let sink = siv.cb_sink().clone();

    thread::spawn(move || {
        let mut prev_idx = usize::MAX;
        let mut prev_paused = false;
        let mut prev_time_text = String::new();

        loop {
            thread::sleep(Duration::from_millis(150));

            let Ok(mut mp) = player_for_thread.lock() else { break };

            let current_idx = mp.current_index;
            let is_paused = mp.is_paused;
            let (current_sec, total_sec) = mp.get_current_progress();

            if current_sec >= total_sec && total_sec > 0 {
                mp.advance_track();
            }

            let index_changed = current_idx != prev_idx;
            if index_changed || is_paused != prev_paused {
                if let Some(track) = mp.queue.get(current_idx) {
                    let t = &track.metadata;
                    utils::update_terminal_title(&t.title, &t.artist, &t.album, &t.year, is_paused);
                }
                prev_idx = current_idx;
                prev_paused = is_paused;
            }

            let track_text = mp.current_track_info().to_string();
            let time_text = format!("{} / {}", utils::format_time(current_sec), utils::format_time(total_sec));
            let indicator_text = if is_paused { "⏸ " } else { "▶ " };
            let repeat_text = match mp.repeat_mode {
                RepeatMode::Off => "[Repeat: Off]",
                RepeatMode::One => "[Repeat: One]",
                RepeatMode::Album => "[Repeat: Album]",
                RepeatMode::Library => "[Repeat: Library]",
            };

            if time_text == prev_time_text && !index_changed {
                drop(mp);
                continue;
            }
            prev_time_text = time_text.clone();

            drop(mp);

            let _ = sink.send(Box::new(move |s| {
                s.call_on_name("play_indicator", |v: &mut TextView| v.set_content(indicator_text));
                s.call_on_name("track_info", |v: &mut TextView| v.set_content(track_text));
                s.call_on_name("time_label", |v: &mut TextView| v.set_content(time_text));
                s.call_on_name("repeat_label", |v: &mut TextView| v.set_content(repeat_text));

                if index_changed {
                    if let Some(mut scroll_view) = s.find_name::<ScrollView<SelectView<TreeItemKey>>>("library_view") {
                        let sv = scroll_view.get_inner_mut();
                        let target_key = TreeItemKey::Track(current_idx);

                        if let Some(pos) = (0..sv.len()).position(|i| sv.get_item(i).is_some_and(|(_, k)| *k == target_key)) {
                            sv.set_selection(pos);
                        }
                    }
                }
            }));
        }
    });

    let player_for_meta = Arc::clone(&music_player);
    siv.add_global_callback('m', move |s| {
        if let Ok(mp) = player_for_meta.lock() {
            if let Some(track) = mp.queue.get(mp.current_index) {
                let m = &track.metadata;
                let meta_text = format!(
                    "Title: {}\nArtist: {}\nAlbum Artist: {}\nAlbum: {}\nYear: {}\nTrack Number: {}\nDuration: {}\nPath: {}",
                    m.title,
                    m.artist,
                    m.album_artist,
                    m.album,
                    m.year,
                    m.track_number,
                    utils::format_time(m.duration.as_secs() as usize),
                    track.path.display()
                );

                s.add_layer(Dialog::around(TextView::new(meta_text)).title(format!("Track Metadata for {}", m.title)).button("Close", |s| {
                    s.pop_layer();
                }));
            }
        }
    });

    let player_for_c = Arc::clone(&music_player);
    siv.add_global_callback('c', move |_| {
        if let Ok(mut mp) = player_for_c.lock() {
            mp.play_pause();
        }
    });

    let player_for_repeat = Arc::clone(&music_player);

    siv.add_global_callback('r', move |_| {
        if let Ok(mut mp) = player_for_repeat.lock() {
            mp.toggle_repeat_mode();
            let mode_str = match mp.repeat_mode {
                RepeatMode::Off => "Off",
                RepeatMode::One => "One",
                RepeatMode::Album => "Album",
                RepeatMode::Library => "Library",
            };
            if let Ok(conn) = rusqlite::Connection::open("rsmus.db") {
                let _ = conn.execute("INSERT OR REPLACE INTO settings (key, value) VALUES ('repeat_mode', ?1)", [&mode_str]);
            }
        }
    });

    let player_for_prev = Arc::clone(&music_player);
    siv.add_global_callback(Event::Key(Key::Left), move |_| {
        if let Ok(mut mp) = player_for_prev.lock() {
            let _ = mp.previous();
        }
    });

    let player_for_next = Arc::clone(&music_player);
    siv.add_global_callback(Event::Key(Key::Right), move |_| {
        if let Ok(mut mp) = player_for_next.lock() {
            let _ = mp.skip();
        }
    });

    siv.add_global_callback('q', |s| s.quit());
    siv.add_global_callback(event::Key::Esc, |s| s.select_menubar());
    siv.run();

    Ok(())
}
