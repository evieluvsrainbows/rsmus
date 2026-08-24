mod dialogs;
pub(crate) mod library;
mod prompts;

use crate::{
    SharedState, TrackHierarchy, db,
    player::{MusicPlayer, RepeatMode},
    utils,
};
use cursive::{
    CbSink, Cursive,
    event::{self, Event, Key},
    menu::Tree as MenuTree,
    theme::Theme,
    view::{Resizable, Scrollable},
    views::{DummyView, LinearLayout, NamedView, Panel, ScrollView, SelectView, TextView},
};
use std::{
    collections::BTreeSet,
    error::Error,
    sync::{
        Arc, Mutex,
        mpsc::{self, Sender},
    },
    thread,
    time::Duration,
};

pub(crate) enum DbTask {
    SaveState(usize, usize),
    SaveSetting(String, String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TreeItemKey {
    Artist(String),
    Album(String, String),
    Track(usize),
}

/// Initializes a background thread to handle all SQLite disk I/O off the UI thread.
fn spawn_db_worker() -> Sender<DbTask> {
    let (tx, rx) = mpsc::channel::<DbTask>();
    thread::spawn(move || {
        let Ok(conn) = rusqlite::Connection::open("rsmus.db") else { return };
        while let Ok(task) = rx.recv() {
            match task {
                DbTask::SaveState(idx, sec) => {
                    let _ = db::save_last_played_state(&conn, idx, sec);
                }
                DbTask::SaveSetting(key, val) => {
                    let _ = conn.execute("INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)", [&key, &val]);
                }
            }
        }
    });
    tx
}

/// Initializes the cursive menubar and sets the theme to be used by it.
pub(crate) fn setup_cursive_theme_and_menu(siv: &mut Cursive, mp: SharedState<MusicPlayer>) {
    let menubar = siv.menubar();
    menubar.add_subtree("File", MenuTree::new().leaf("Quit (q)", |s| s.quit()));
    menubar.add_subtree("Song", MenuTree::new().leaf("Show Metadata\u{2026}", move |s| dialogs::show_metadata(s, mp.clone())));
    siv.set_autohide_menu(false);
    siv.set_theme(Theme::terminal_default());
}

pub(crate) fn setup_ui_layout(
    siv: &mut Cursive,
    hierarchy: &TrackHierarchy,
    mp: SharedState<MusicPlayer>,
    expanded_artists: SharedState<BTreeSet<String>>,
    expanded_albums: SharedState<BTreeSet<(String, String)>>,
) -> Result<(), Box<dyn Error>> {
    let mut select_view = SelectView::<TreeItemKey>::new();
    let hierarchy_for_submit = hierarchy.clone();
    let player_for_submit = mp.clone();

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

    let (current_idx, is_paused, initial_info, initial_sec, initial_duration) = {
        let mp = mp.lock().map_err(|_| "Poisoned mutex")?;
        let cur_idx = mp.current_index;
        let is_p = mp.is_paused;
        let info = mp.current_track_info().to_string();
        let (sec, dur) = mp.get_current_progress();
        (cur_idx, is_p, info, sec, dur)
    };

    let artists_guard = expanded_artists.lock().map_err(|_| "Poisoned mutex")?;
    let albums_guard = expanded_albums.lock().map_err(|_| "Poisoned mutex")?;
    let borrowed_albums: BTreeSet<(&str, &str)> = albums_guard.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();

    library::construct_view(&mut select_view, hierarchy, &artists_guard, &borrowed_albums, current_idx, is_paused, true);

    drop(artists_guard);
    drop(albums_guard);

    let initial_indicator = if is_paused { "⏸ " } else { "▶ " };
    let initial_time_text = format!("{} / {}", utils::format_time(initial_sec), utils::format_time(initial_duration));
    let play_indicator = NamedView::new("play_indicator", TextView::new(initial_indicator));
    let track_info = NamedView::new("track_info", TextView::new(initial_info).no_wrap()).max_height(1);
    let time_label = NamedView::new("time_label", TextView::new(initial_time_text));
    let repeat_label = NamedView::new("repeat_label", TextView::new("[Repeat: Off]"));
    let status_bar = LinearLayout::horizontal()
        .child(play_indicator)
        .child(track_info.max_height(1).full_width())
        .child(DummyView)
        .child(repeat_label)
        .child(TextView::new("  "))
        .child(time_label);

    let library_panel = Panel::new(NamedView::new("library_view", select_view.scrollable())).title("Music Library");
    let prompt_bar = NamedView::new("prompt_bar", TextView::new("")).max_height(1);
    let root_layout = LinearLayout::vertical().child(library_panel.full_height()).child(status_bar).child(prompt_bar).full_screen();

    siv.add_layer(root_layout);

    Ok(())
}

pub(crate) fn handle_repeat_mode(siv: &mut Cursive, mp: Arc<Mutex<MusicPlayer>>) {
    let repeat_sink = siv.cb_sink().clone();
    thread::spawn(move || {
        let mut prev_repeat_mode = None;
        loop {
            thread::sleep(Duration::from_millis(150));

            let Ok(mp) = mp.lock() else { break };
            let current_repeat_mode = mp.repeat_mode;
            drop(mp);

            if Some(current_repeat_mode) == prev_repeat_mode {
                continue;
            }
            prev_repeat_mode = Some(current_repeat_mode);

            let repeat_text = match current_repeat_mode {
                RepeatMode::Off => "[Repeat: Off]",
                RepeatMode::Single => "[Repeat: Single]",
                RepeatMode::Album => "[Repeat: Album]",
                RepeatMode::All => "[Repeat: All]",
            };

            let _ = repeat_sink.send(Box::new(move |s| {
                s.call_on_name("repeat_label", |v: &mut TextView| v.set_content(repeat_text));
            }));
        }
    });
}

pub(crate) fn register_callbacks(
    siv: &mut Cursive,
    mp: Arc<Mutex<MusicPlayer>>,
    hierarchy: &TrackHierarchy,
    expanded_artists: SharedState<BTreeSet<String>>,
    expanded_albums: SharedState<BTreeSet<(String, String)>>,
) {
    let db_tx = spawn_db_worker();

    let artists_for_space = expanded_artists.clone();
    let albums_for_space = expanded_albums.clone();
    let hierarchy_for_space = hierarchy.clone();
    let player_for_space = mp.clone();

    siv.add_global_callback(' ', move |s| {
        if let Some(mut scroll_view) = s.find_name::<ScrollView<SelectView<TreeItemKey>>>("library_view") {
            let sv = scroll_view.get_inner_mut();
            if let Some(item_key) = sv.selection().map(|v| (*v).clone()) {
                let (cur_idx, is_paused) = player_for_space.lock().map(|mp| (mp.current_index, mp.is_paused)).unwrap_or((0, false));
                match item_key {
                    TreeItemKey::Artist(artist_name) => {
                        let Ok(mut artists) = artists_for_space.lock() else { return };
                        let Ok(albums_guard) = albums_for_space.lock() else { return };

                        if artists.contains(&artist_name) {
                            artists.remove(&artist_name);
                        } else {
                            artists.insert(artist_name);
                        }

                        let borrowed_albums: BTreeSet<(&str, &str)> = albums_guard.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
                        library::construct_view(sv, &hierarchy_for_space, &artists, &borrowed_albums, cur_idx, is_paused, false);
                    }
                    TreeItemKey::Album(artist_name, album_name) => {
                        let Ok(artists_guard) = artists_for_space.lock() else { return };
                        let Ok(mut albums) = albums_for_space.lock() else { return };

                        let album_key = (artist_name, album_name);
                        if albums.contains(&album_key) {
                            albums.remove(&album_key);
                        } else {
                            albums.insert(album_key);
                        }

                        let borrowed_albums: BTreeSet<(&str, &str)> = albums.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
                        library::construct_view(sv, &hierarchy_for_space, &artists_guard, &borrowed_albums, cur_idx, is_paused, false);
                    }
                    TreeItemKey::Track(_) => {}
                }
            }
        }
    });

    // Key binding for opening the track metadata dialog.
    let player_for_metadata = mp.clone();
    siv.add_global_callback('m', move |s| dialogs::show_metadata(s, player_for_metadata.clone()));

    // Key binding for playing or pausing the current track.
    let player_for_playback = mp.clone();
    siv.add_global_callback('c', move |_| {
        if let Ok(mut mp) = player_for_playback.lock() {
            mp.play_pause();
        }
    });

    // Key binding for toggling the repeat state.
    let player_for_repeat = mp.clone();
    siv.add_global_callback('r', move |_| {
        if let Ok(mut mp) = player_for_repeat.lock() {
            mp.toggle_repeat_mode();
            let mode_str = match mp.repeat_mode {
                RepeatMode::Off => "Off",
                RepeatMode::Single => "Single",
                RepeatMode::Album => "Album",
                RepeatMode::All => "All",
            };
            let _ = db_tx.send(DbTask::SaveSetting("repeat_mode".into(), mode_str.into()));
        }
    });

    // Key binding for rewinding a track by 10 seconds.
    let player_for_rewind = mp.clone();
    siv.add_global_callback('b', move |_| {
        if let Ok(mut mp) = player_for_rewind.lock() {
            let _ = mp.seek_backward();
        }
    });

    // Key binding for fast forwarding a track by 10 seconds.
    let player_for_forward = mp.clone();
    siv.add_global_callback('n', move |_| {
        if let Ok(mut mp) = player_for_forward.lock() {
            let _ = mp.seek_forward();
        }
    });

    // Key binding for rewinding to the previous track.
    let player_for_prev = mp.clone();
    siv.add_global_callback(Event::Key(Key::Left), move |_| {
        if let Ok(mut mp) = player_for_prev.lock() {
            let _ = mp.previous();
        }
    });

    // Key binding for skipping to the next track.
    siv.add_global_callback(Event::Key(Key::Right), move |_| {
        if let Ok(mut mp) = mp.lock() {
            let _ = mp.skip();
        }
    });

    siv.add_global_callback('q', |s| prompts::show_quit_prompt(s));
    siv.add_global_callback(event::Key::Esc, |s| s.select_menubar());
}

pub(crate) fn spawn_playback_thread(
    sink: CbSink,
    mp: SharedState<MusicPlayer>,
    hierarchy: TrackHierarchy,
    expanded_artists: SharedState<BTreeSet<String>>,
    expanded_albums: SharedState<BTreeSet<(String, String)>>,
) {
    let player_for_thread = mp.clone();
    let artists_for_thread = expanded_artists.clone();
    let albums_for_thread = expanded_albums.clone();
    let hierarchy_for_thread = hierarchy;
    let db_tx = spawn_db_worker();

    thread::spawn(move || {
        let mut prev_idx = usize::MAX;
        let mut prev_paused = false;
        let mut prev_time_text = String::new();
        let mut has_advanced = false;

        loop {
            thread::sleep(Duration::from_millis(250));

            let Ok(mut mp) = player_for_thread.lock() else {
                break;
            };

            let (current_sec, total_sec) = mp.get_current_progress();
            let _ = db_tx.send(DbTask::SaveState(mp.current_index, current_sec));

            if current_sec >= total_sec && total_sec > 0 {
                if !has_advanced {
                    mp.advance_track();
                    has_advanced = true;
                }
            } else {
                has_advanced = false;
            }

            let current_idx = mp.current_index;
            let is_paused = mp.is_paused;
            let index_changed = current_idx != prev_idx;
            let paused_changed = is_paused != prev_paused;

            if index_changed || paused_changed {
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

            if time_text == prev_time_text && !index_changed && !paused_changed {
                drop(mp);
                continue;
            }

            prev_time_text = time_text.clone();
            drop(mp);

            let Ok(artists_guard) = artists_for_thread.lock() else { continue };
            let Ok(albums_guard) = albums_for_thread.lock() else { continue };

            let artists_snapshot = artists_guard.clone();
            let albums_snapshot = albums_guard.clone();
            let hierarchy_snapshot = hierarchy_for_thread.clone();

            drop(artists_guard);
            drop(albums_guard);

            let (view_items, selection_idx) = if index_changed || paused_changed {
                let borrowed_albums: BTreeSet<(&str, &str)> = albums_snapshot.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
                let (items, sel) = library::generate_items(&hierarchy_snapshot, &artists_snapshot, &borrowed_albums, current_idx, is_paused, index_changed, None);
                (Some(items), sel)
            } else {
                (None, None)
            };

            let _ = sink.send(Box::new(move |s| {
                s.call_on_name("play_indicator", |v: &mut TextView| v.set_content(indicator_text));
                s.call_on_name("track_info", |v: &mut TextView| v.set_content(track_text));
                s.call_on_name("time_label", |v: &mut TextView| v.set_content(time_text));

                if let Some(items) = view_items {
                    if let Some(mut scroll_view) = s.find_name::<ScrollView<SelectView<TreeItemKey>>>("library_view") {
                        let sv = scroll_view.get_inner_mut();
                        sv.clear();
                        for (label, key) in items {
                            sv.add_item(label, key);
                        }
                        if let Some(idx) = selection_idx {
                            sv.set_selection(idx);
                        }
                    }
                }
            }));
        }
    });
}
