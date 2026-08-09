use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use cursive::views::SelectView;

use crate::{player::Track, utils};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TreeItemKey {
    Artist(String),
    Album(String, String),
    Track(usize),
}

pub fn rebuild_library_view(
    select_view: &mut SelectView<TreeItemKey>,
    hierarchy: &BTreeMap<String, BTreeMap<String, Vec<(usize, Track)>>>,
    expanded_artists: &BTreeSet<String>,
    expanded_albums: &BTreeSet<(String, String)>,
) {
    let current_selection = select_view.selection();

    select_view.clear();

    let mut target_index = None;
    let mut index_counter = 0;

    for (album_artist, albums) in hierarchy {
        let artist_expanded = expanded_artists.contains(album_artist);
        let artist_icon = if artist_expanded { "▼" } else { "▶" };

        let artist_key = TreeItemKey::Artist(album_artist.clone());
        if current_selection.as_ref() == Some(&Arc::new(artist_key.clone())) {
            target_index = Some(index_counter);
        }

        select_view.add_item(format!("{} {}", artist_icon, album_artist), artist_key);
        index_counter += 1;

        if !artist_expanded {
            continue;
        }

        let album_count = albums.len();
        for (a_idx, (album, unsorted_tracks)) in albums.iter().enumerate() {
            let is_last_album = a_idx == album_count - 1;
            let album_branch = if is_last_album { "└──" } else { "├──" };
            let child_prefix = if is_last_album { "    " } else { "│   " };

            let album_key_tuple = (album_artist.clone(), album.clone());
            let album_expanded = expanded_albums.contains(&album_key_tuple);
            let album_icon = if album_expanded { "▼" } else { "▶" };

            let year = unsorted_tracks.first().map(|(_, t)| t.metadata.year.as_str()).unwrap_or("Unknown Year");

            let album_key = TreeItemKey::Album(album_artist.clone(), album.clone());
            if current_selection.as_ref() == Some(&Arc::new(album_key.clone())) {
                target_index = Some(index_counter);
            }

            select_view.add_item(format!("{} {} {} ({})", album_branch, album_icon, album, year), album_key);
            index_counter += 1;

            if !album_expanded {
                continue;
            }

            // Sort tracks numerically by track_number to prevent alphabetical sorting issues (e.g., "12" before "10")
            let mut tracks = unsorted_tracks.clone();
            tracks.sort_by(|(_, a), (_, b)| a.metadata.track_number.cmp(&b.metadata.track_number));

            let track_count = tracks.len();
            for (t_idx, (global_idx, track)) in tracks.iter().enumerate() {
                let is_last_track = t_idx == track_count - 1;
                let track_branch = if is_last_track { "└──" } else { "├──" };
                let m = &track.metadata;
                let duration_str = utils::format_time(m.duration.as_secs() as usize);

                let track_key = TreeItemKey::Track(*global_idx);
                if current_selection.as_ref() == Some(&Arc::new(track_key.clone())) {
                    target_index = Some(index_counter);
                }

                let track_line = if m.album_artist != m.artist {
                    format!("{}    {} {}. {} ({}) [{}]", child_prefix, track_branch, m.track_number, m.title, m.artist, duration_str)
                } else {
                    format!("{}    {} {}. {} [{}]", child_prefix, track_branch, m.track_number, m.title, duration_str)
                };

                select_view.add_item(track_line, track_key);
                index_counter += 1;
            }
        }
    }

    if let Some(idx) = target_index {
        select_view.set_selection(idx);
    }
}
