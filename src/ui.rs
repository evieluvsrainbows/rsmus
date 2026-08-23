use crate::{player::Track, utils};
use cursive::{
    event,
    theme::{Color, Style},
    utils::markup::StyledString,
    views::{SelectView, TextView},
};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TreeItemKey {
    Artist(String),
    Album(String, String),
    Track(usize),
}

/// Constructs the library view and its structure.
pub(crate) fn construct_library_view(
    select_view: &mut SelectView<TreeItemKey>,
    hierarchy: &BTreeMap<String, BTreeMap<String, Vec<(usize, Track)>>>,
    expanded_artists: &BTreeSet<String>,
    expanded_albums: &BTreeSet<(String, String)>,
    current_track_idx: usize,
    is_paused: bool,
) {
    let binding = select_view.selection();
    let selected_key = binding.as_deref().map(|arc| &*arc);

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

            let album_expanded = expanded_albums.contains(&(album_artist.clone(), album.clone()));
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
                if selected_key == Some(&track_key) {
                    target_index = Some(index_counter);
                }

                let icon = if *global_idx == current_track_idx { if is_paused { "⏸ " } else { "♫ " } } else { "" };
                let track_line = if m.album_artist != m.artist {
                    format!("{child_prefix}    {track_branch} {}. {icon}{} ({}) [{duration_str}]", m.track_number, m.title, m.artist)
                } else {
                    format!("{child_prefix}    {track_branch} {}. {icon}{} [{duration_str}]", m.track_number, m.title)
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

pub(crate) fn show_quit_prompt(siv: &mut cursive::Cursive) {
    siv.call_on_name("prompt_bar", |v: &mut TextView| {
        let style = Style::from(Color::Rgb(255, 238, 140));
        v.set_content(StyledString::styled("Quit [y/N]?", style));
    });

    siv.add_global_callback('y', |s| s.quit());
    siv.add_global_callback('Y', |s| s.quit());

    let cancel_prompt = |s: &mut cursive::Cursive| {
        s.clear_global_callbacks('y');
        s.clear_global_callbacks('Y');
        s.clear_global_callbacks('n');
        s.clear_global_callbacks('N');

        s.call_on_name("prompt_bar", |v: &mut TextView| {
            v.set_content("");
        });
    };

    siv.add_global_callback('n', cancel_prompt);
    siv.add_global_callback('N', cancel_prompt);
    siv.add_global_callback(event::Key::Esc, cancel_prompt);
}
