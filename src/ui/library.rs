use cursive::{
    theme::{Effect, Style},
    utils::markup::StyledString,
    views::SelectView,
};
use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
    error::Error,
    sync::{Arc, Mutex},
};

use crate::{
    SharedState, TrackHierarchy,
    player::{MusicPlayer, Track},
    ui::TreeItemKey,
    utils,
};

type ExpandedStates = (SharedState<BTreeSet<String>>, SharedState<BTreeSet<(String, String)>>);

pub(crate) fn get_initial_expanded_states(hierarchy: &TrackHierarchy) -> ExpandedStates {
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

pub(crate) fn generate_items(
    hierarchy: &TrackHierarchy,
    expanded_artists: &BTreeSet<String>,
    expanded_albums: &BTreeSet<(&str, &str)>,
    current_track_idx: usize,
    is_paused: bool,
    force_highlight_current: bool,
    selected_key: Option<&TreeItemKey>,
) -> (Vec<(StyledString, TreeItemKey)>, Option<usize>) {
    let mut items = Vec::new();
    let mut target_index = None;
    let mut current_track_view_index = None;
    let mut index_counter = 0;

    for (album_artist, albums) in hierarchy {
        let artist_expanded = expanded_artists.contains(album_artist);
        let artist_icon = if artist_expanded { "▼" } else { "▶" };
        let artist_key = TreeItemKey::Artist(album_artist.clone());
        if !force_highlight_current && selected_key == Some(&artist_key) {
            target_index = Some(index_counter);
        }

        items.push((StyledString::plain(format!("{artist_icon} {album_artist}")), artist_key));
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
            if !force_highlight_current && selected_key == Some(&album_key) {
                target_index = Some(index_counter);
            }

            items.push((StyledString::plain(format!("{album_branch} {album_icon} {album} ({year})")), album_key));
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
                if is_current_track {
                    current_track_view_index = Some(index_counter);
                } else if !force_highlight_current && selected_key == Some(&track_key) {
                    target_index = Some(index_counter);
                }

                let icon = if is_current_track { if is_paused { "⏸ " } else { "▶ " } } else { "" };
                let artist_extra = if m.album_artist != m.artist { format!(" ({})", m.artist) } else { String::new() };

                let prefix = format!("{child_prefix}    {track_branch} {}. {icon}", m.track_number);
                let suffix = format!("{artist_extra} [{duration_str}]");

                let mut styled_label = StyledString::plain(&prefix);
                if is_current_track {
                    styled_label.append_styled(&m.title, Style::from(Effect::Bold));
                } else {
                    styled_label.append_plain(&m.title);
                }
                styled_label.append_plain(&suffix);

                items.push((styled_label, track_key));
                index_counter += 1;
            }
        }
    }

    let final_selection = if force_highlight_current {
        current_track_view_index.or(target_index)
    } else {
        target_index.or(current_track_view_index)
    };

    (items, final_selection)
}

pub(crate) fn construct_view(
    select_view: &mut SelectView<TreeItemKey>,
    hierarchy: &TrackHierarchy,
    expanded_artists: &BTreeSet<String>,
    expanded_albums: &BTreeSet<(&str, &str)>,
    current_track_idx: usize,
    is_paused: bool,
    force_highlight_current: bool,
) {
    let selection = select_view.selection();
    let selected_key = selection.as_deref();

    let (items, selection_idx) = generate_items(hierarchy, expanded_artists, expanded_albums, current_track_idx, is_paused, force_highlight_current, selected_key);

    select_view.clear();
    for (label, key) in items {
        select_view.add_item(label, key);
    }

    if let Some(idx) = selection_idx {
        select_view.set_selection(idx);
    }
}

pub(crate) fn build_hierarchy(mp: &SharedState<MusicPlayer>) -> Result<TrackHierarchy, Box<dyn Error + Send + Sync>> {
    let mut hierarchy: TrackHierarchy = BTreeMap::new();
    {
        let mp = mp.lock().map_err(|_| "Mutex poisoned")?;
        for (i, track) in mp.queue.iter().enumerate() {
            let album_artist = if track.metadata.album_artist.is_empty() {
                Cow::Borrowed(track.metadata.artist.as_str())
            } else {
                Cow::Borrowed(track.metadata.album_artist.as_str())
            };

            hierarchy
                .entry(album_artist.into_owned())
                .or_default()
                .entry(track.metadata.album.clone())
                .or_default()
                .push((i, track.clone()));
        }
    }

    for albums in hierarchy.values_mut() {
        for tracks in albums.values_mut() {
            tracks.sort_unstable_by_key(|(_, track)| track.metadata.track_number);
        }
    }

    Ok(hierarchy)
}
