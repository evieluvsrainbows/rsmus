use crate::{
    SharedState, TrackHierarchy,
    player::{MusicPlayer, RepeatMode, Track},
    ui_utils, utils,
};
use cursive::{
    CbSink, Cursive,
    event::{self, Event, Key},
    menu,
    theme::Theme,
    view::{Resizable, Scrollable},
    views::{Dialog, DummyView, LinearLayout, NamedView, Panel, ScrollView, SelectView, TextView},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TreeItemKey {
    Artist(String),
    Album(String, String),
    Track(usize),
}

/// Initializes the cursive menubar and sets the theme to be used by it.
pub(crate) fn setup_cursive_theme_and_menu(siv: &mut Cursive) {
    siv.menubar().add_subtree("File", menu::Tree::new().leaf("Quit", |s| s.quit()));
    siv.set_autohide_menu(false);
    siv.set_theme(Theme::terminal_default());
}

pub(crate) fn get_initial_expanded_states(hierarchy: &TrackHierarchy) -> (SharedState<BTreeSet<String>>, SharedState<BTreeSet<(String, String)>>) {
    let mut initial_artists = BTreeSet::new();
    let mut initial_albums = BTreeSet::new();
    for (artist, albums) in hierarchy {
        initial_artists.insert(artist.clone());
        for album in albums.keys() {
            initial_albums.insert((artist.clone(), album.clone()));
        }
    }

    (Arc::new(Mutex::new(initial_artists)), Arc::new(Mutex::new(initial_albums)))
}

/// Constructs the library view and its structure. This function is called
/// whenever the library view needs to update, e.g. when changing tracks or
/// collapsing or expanding an artist or album.
pub(crate) fn construct_library_view(
    select_view: &mut SelectView<TreeItemKey>,
    hierarchy: &BTreeMap<String, BTreeMap<String, Vec<(usize, Track)>>>,
    expanded_artists: &BTreeSet<String>,
    expanded_albums: &BTreeSet<(&str, &str)>, // Borrowed tuple keys avoid allocations on lookup
    current_track_idx: usize,
    is_paused: bool,
) {
    let selection = select_view.selection();
    let selected_key = selection.as_deref().map(|key| key);
    select_view.clear();

    let mut target_index = None;
    let mut index_counter = 0;

    for (album_artist, albums) in hierarchy {
        let artist_expanded = expanded_artists.contains(album_artist);
        let artist_icon = if artist_expanded { "▼" } else { "▶" };

        let artist_key = TreeItemKey::Artist(album_artist.clone());
        if selected_key == Some(&artist_key) {
            target_index = Some(index_counter);
        }

        select_view.add_item(format!("{artist_icon} {album_artist}"), artist_key);
        index_counter += 1;

        if !artist_expanded {
            continue;
        }

        let album_count = albums.len();
        for (a_idx, (album, unsorted_tracks)) in albums.iter().enumerate() {
            let is_last_album = a_idx == album_count - 1;
            let album_branch = if is_last_album { "└──" } else { "├──" };
            let child_prefix = if is_last_album { "    " } else { "│   " };

            let album_expanded = expanded_albums.contains(&(album_artist.as_str(), album.as_str()));
            let album_icon = if album_expanded { "▼" } else { "▶" };

            let year = unsorted_tracks.first().map(|(_, t)| t.metadata.year.as_str()).unwrap_or("Unknown Year");

            let album_key = TreeItemKey::Album(album_artist.clone(), album.clone());
            if selected_key == Some(&album_key) {
                target_index = Some(index_counter);
            }

            select_view.add_item(format!("{album_branch} {album_icon} {album} ({year})"), album_key);
            index_counter += 1;

            if !album_expanded {
                continue;
            }

            let mut track_refs: Vec<&(usize, Track)> = unsorted_tracks.iter().collect();
            track_refs.sort_unstable_by_key(|(_, t)| t.metadata.track_number);

            let track_count = track_refs.len();
            for (t_idx, (global_idx, track)) in track_refs.into_iter().enumerate() {
                let is_last_track = t_idx == track_count - 1;
                let track_branch = if is_last_track { "└──" } else { "├──" };
                let m = &track.metadata;
                let duration_str = utils::format_time(m.duration.as_secs() as usize);

                let track_key = TreeItemKey::Track(*global_idx);
                let is_current_track = *global_idx == current_track_idx;

                if selected_key == Some(&track_key) {
                    target_index = Some(index_counter);
                } else if is_current_track && target_index.is_none() {
                    target_index = Some(index_counter);
                }

                let icon = if is_current_track { if is_paused { "⏸ " } else { "♫ " } } else { "" };

                let artist_extra = if m.album_artist != m.artist { format!(" ({})", m.artist) } else { String::new() };

                let track_line = format!("{child_prefix}    {track_branch} {}. {icon}{}{artist_extra} [{duration_str}]", m.track_number, m.title);

                select_view.add_item(track_line, track_key);
                index_counter += 1;
            }
        }
    }

    if let Some(idx) = target_index {
        select_view.set_selection(idx);
    }
}

pub(crate) fn setup_ui_layout(
    siv: &mut Cursive,
    hierarchy: &TrackHierarchy,
    music_player: SharedState<MusicPlayer>,
    expanded_artists: SharedState<BTreeSet<String>>,
    expanded_albums: SharedState<BTreeSet<(String, String)>>,
) -> Result<(), Box<dyn Error>> {
    let mut select_view = SelectView::<TreeItemKey>::new();
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

    let artists_guard = expanded_artists.lock().unwrap();
    let albums_guard = expanded_albums.lock().unwrap();
    let borrowed_albums: BTreeSet<(&str, &str)> = albums_guard.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();

    construct_library_view(&mut select_view, hierarchy, &artists_guard, &borrowed_albums, 0, false);

    drop(artists_guard);
    drop(albums_guard);

    let (initial_info, initial_duration) = {
        let mp = music_player.lock().map_err(|_| "Poisoned mutex")?;
        let info = mp.current_track_info().to_string();
        let duration = mp.queue.get(0).map(|t| t.metadata.duration.as_secs() as usize).unwrap_or(100);
        (info, duration)
    };

    let initial_time_text = format!("0:00 / {}", utils::format_time(initial_duration));
    let play_indicator = NamedView::new("play_indicator", TextView::new("▶ "));
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

pub(crate) fn build_hierarchy(music_player: &SharedState<MusicPlayer>) -> Result<TrackHierarchy, Box<dyn Error>> {
    let mut hierarchy: TrackHierarchy = BTreeMap::new();
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

    for albums in hierarchy.values_mut() {
        for tracks in albums.values_mut() {
            tracks.sort_by_key(|(_, track)| track.metadata.track_number);
        }
    }

    Ok(hierarchy)
}

pub(crate) fn handle_repeat_mode(siv: &mut Cursive, music_player: Arc<Mutex<MusicPlayer>>) {
    let repeat_sink = siv.cb_sink().clone();

    thread::spawn(move || {
        let mut prev_repeat_mode = None;
        loop {
            thread::sleep(Duration::from_millis(150));

            let Ok(mp) = music_player.lock() else { break };
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
    music_player: Arc<Mutex<MusicPlayer>>,
    hierarchy: &TrackHierarchy,
    expanded_artists: SharedState<BTreeSet<String>>,
    expanded_albums: SharedState<BTreeSet<(String, String)>>,
) {
    let artists_for_space = Arc::clone(&expanded_artists);
    let albums_for_space = Arc::clone(&expanded_albums);
    let hierarchy_for_space = hierarchy.clone();
    let player_for_space = Arc::clone(&music_player);
    siv.add_global_callback(' ', move |s| {
        if let Some(mut scroll_view) = s.find_name::<ScrollView<SelectView<TreeItemKey>>>("library_view") {
            let sv = scroll_view.get_inner_mut();
            if let Some(item_key) = sv.selection().map(|v| (*v).clone()) {
                let (cur_idx, is_paused) = player_for_space.lock().map(|mp| (mp.current_index, mp.is_paused)).unwrap_or((0, false));
                match item_key {
                    TreeItemKey::Artist(artist_name) => {
                        let mut artists = artists_for_space.lock().unwrap();
                        if artists.contains(&artist_name) {
                            artists.remove(&artist_name);
                        } else {
                            artists.insert(artist_name);
                        }

                        let albums_guard = albums_for_space.lock().unwrap();
                        let borrowed_albums: BTreeSet<(&str, &str)> = albums_guard.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();

                        construct_library_view(sv, &hierarchy_for_space, &artists, &borrowed_albums, cur_idx, is_paused);
                    }
                    TreeItemKey::Album(artist_name, album_name) => {
                        let album_key = (artist_name, album_name);
                        let mut albums = albums_for_space.lock().unwrap();
                        if albums.contains(&album_key) {
                            albums.remove(&album_key);
                        } else {
                            albums.insert(album_key);
                        }

                        let artists_guard = artists_for_space.lock().unwrap();
                        let borrowed_albums: BTreeSet<(&str, &str)> = albums.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();

                        construct_library_view(sv, &hierarchy_for_space, &artists_guard, &borrowed_albums, cur_idx, is_paused);
                    }
                    TreeItemKey::Track(_) => {}
                }
            }
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
                s.add_layer(Dialog::around(TextView::new(meta_text)).title(format!("{} - Metadata", m.title)).button("Close", |s| {
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
                RepeatMode::Single => "Single",
                RepeatMode::Album => "Album",
                RepeatMode::All => "All",
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

    siv.add_global_callback(Event::Key(Key::Right), move |_| {
        if let Ok(mut mp) = music_player.lock() {
            let _ = mp.skip();
        }
    });

    siv.add_global_callback('q', |s| ui_utils::show_quit_prompt(s));
    siv.add_global_callback(event::Key::Esc, |s| s.select_menubar());
}

pub(crate) fn spawn_playback_thread(
    sink: CbSink,
    music_player: SharedState<MusicPlayer>,
    hierarchy: TrackHierarchy,
    expanded_artists: SharedState<BTreeSet<String>>,
    expanded_albums: SharedState<BTreeSet<(String, String)>>,
) {
    let player_for_thread = Arc::clone(&music_player);
    let artists_for_thread = Arc::clone(&expanded_artists);
    let albums_for_thread = Arc::clone(&expanded_albums);
    let hierarchy_for_thread = hierarchy;

    thread::spawn(move || {
        let mut prev_idx = usize::MAX;
        let mut prev_paused = false;
        let mut prev_time_text = String::new();

        loop {
            thread::sleep(Duration::from_millis(250));

            let Ok(mut mp) = player_for_thread.lock() else {
                break;
            };

            let current_idx = mp.current_index;
            let is_paused = mp.is_paused;
            let (current_sec, total_sec) = mp.get_current_progress();

            if current_sec >= total_sec && total_sec > 0 {
                mp.advance_track();
            }

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

            let artists_snapshot = artists_for_thread.lock().unwrap().clone();
            let albums_snapshot = albums_for_thread.lock().unwrap().clone();
            let hierarchy_snapshot = hierarchy_for_thread.clone();

            let _ = sink.send(Box::new(move |s| {
                s.call_on_name("play_indicator", |v: &mut TextView| v.set_content(indicator_text));
                s.call_on_name("track_info", |v: &mut TextView| v.set_content(track_text));
                s.call_on_name("time_label", |v: &mut TextView| v.set_content(time_text));
                if index_changed || paused_changed {
                    if let Some(mut scroll_view) = s.find_name::<ScrollView<SelectView<TreeItemKey>>>("library_view") {
                        let sv = scroll_view.get_inner_mut();
                        let borrowed_albums: BTreeSet<(&str, &str)> = albums_snapshot.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
                        construct_library_view(sv, &hierarchy_snapshot, &artists_snapshot, &borrowed_albums, current_idx, is_paused);
                    }
                }
            }));
        }
    });
}
