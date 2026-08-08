mod player;
mod utils;

use audiotags::Tag;
use clap::Parser;
use cursive::{
    event::{self, Event, Key},
    menu,
    theme::Theme,
    view::Resizable,
    views::{Dialog, NamedView, TextView},
};
use rayon::prelude::*;
use rodio::{Decoder, Source};
use std::{
    error::Error,
    fs::File,
    path::PathBuf,
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use player::{MusicPlayer, TrackMetadata};

#[derive(Parser, Debug)]
#[clap(about, version)]
struct Args {
    /// Album or individual song to play. Required.
    #[arg(required = true, index = 1)]
    input: String,
}

fn main() -> Result<(), Box<dyn Error>> {
    let args = Args::parse();

    let mut siv = cursive::default();
    siv.menubar()
        .add_subtree("File", menu::Tree::new().leaf("Quit", |s| s.quit()))
        .add_subtree("Help", menu::Tree::new().leaf("About", |s| s.add_layer(Dialog::info("rsmus 0.0.1").title("About rsmus"))));
    siv.set_autohide_menu(false);
    siv.set_theme(Theme::terminal_default());

    let input = args.input;
    let input_path = PathBuf::from(&input);

    let supported_extension = |path: &std::path::Path| -> bool {
        path.extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| {
                let lower = ext.to_lowercase();
                lower == "flac" || lower == "mp3" || lower == "m4a" || lower == "wav" || lower == "ogg"
            })
            .unwrap_or(false)
    };

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
                album: tag.album_title().unwrap_or("Unknown Album").to_string(),
                year: tag.year().map(|y| y.to_string()).unwrap_or_else(|| "Unknown Year".to_string()),
                duration,
            }
        } else {
            TrackMetadata {
                title: utils::clean_title(&fallback_name),
                artist: "Unknown Artist".to_string(),
                album: "Unknown Album".to_string(),
                year: "Unknown Year".to_string(),
                duration,
            }
        }
    };

    let mut paths_to_append = Vec::new();

    if std::fs::metadata(&input_path)?.is_dir() {
        let entries: Vec<PathBuf> = std::fs::read_dir(&input_path)?
            .filter_map(Result::ok)
            .map(|f| f.path())
            .filter(|path| path.is_file() && supported_extension(path))
            .collect();

        if entries.is_empty() {
            return Err(format!("No supported music files found in directory: {}", input).into());
        }

        let mut sorted_entries = entries;
        sorted_entries.sort();
        paths_to_append = sorted_entries;
    } else {
        if !supported_extension(&input_path) {
            return Err(format!("Specified file is not a supported music file: {}", input).into());
        }
        paths_to_append.push(input_path);
    }

    let track_metadata_list: Vec<TrackMetadata> = paths_to_append.par_iter().map(|filepath| extract_metadata(filepath)).collect();
    let music_player = Arc::new(Mutex::new(MusicPlayer::new(track_metadata_list, paths_to_append)?));
    {
        let mut mp = music_player.lock().unwrap();
        for filepath in &mp.paths {
            let file = File::open(filepath)?;
            let source = Decoder::try_from(file)?;
            mp.player.append(source);
        }
        mp.track_start_time = Instant::now();

        if let Some(track) = mp.tracks.get(mp.current_index) {
            utils::update_terminal_title(&track.title, &track.artist, &track.album, &track.year, mp.is_paused);
        }
    }

    let initial_info = music_player.lock().unwrap().current_track_info();
    let initial_duration = music_player.lock().unwrap().tracks.get(0).map(|t| t.duration.as_secs() as usize).unwrap_or(100);
    let initial_time_text = format!("0:00 / {}", utils::format_time(initial_duration));

    let indicator_label = NamedView::new("play_indicator", TextView::new("▶ "));
    let track_label = NamedView::new("track_info", TextView::new(format!("{}", initial_info)));
    let time_label = NamedView::new("time_label", TextView::new(initial_time_text));

    let status_bar = cursive::views::LinearLayout::horizontal()
        .child(indicator_label)
        .child(track_label)
        .child(cursive::views::DummyView.full_width())
        .child(time_label);

    let root_layout = cursive::views::LinearLayout::vertical()
        .child(cursive::views::DummyView.full_height())
        .child(status_bar)
        .full_screen();

    siv.add_layer(root_layout);

    let player_for_thread = Arc::clone(&music_player);
    let sink = siv.cb_sink().clone();
    thread::spawn(move || {
        let mut previous_index = usize::MAX;
        let mut previous_paused = false;

        loop {
            thread::sleep(Duration::from_millis(150));

            let mut mp = player_for_thread.lock().unwrap();
            let current_idx = mp.current_index;
            let current_paused = mp.is_paused;
            let (current_sec, total_sec) = mp.get_current_progress();

            if current_sec >= total_sec && total_sec > 0 && current_idx < mp.tracks.len() - 1 {
                mp.advance_track();
            }

            if current_idx != previous_index || current_paused != previous_paused {
                if let Some(track) = mp.tracks.get(current_idx) {
                    utils::update_terminal_title(&track.title, &track.artist, &track.album, &track.year, current_paused);
                }
                previous_index = current_idx;
                previous_paused = current_paused;
            }

            let track_text = format!("{}", mp.current_track_info());
            let (current, total) = mp.get_current_progress();
            let time_text = format!("{} / {}", utils::format_time(current), utils::format_time(total));
            let indicator_text = if current_paused { "⏸ " } else { "▶ " };

            let _ = sink.send(Box::new(move |s| {
                s.call_on_name("play_indicator", |view: &mut TextView| {
                    view.set_content(indicator_text);
                });
                s.call_on_name("track_info", |view: &mut TextView| {
                    view.set_content(track_text);
                });
                s.call_on_name("time_label", |view: &mut TextView| {
                    view.set_content(time_text);
                });
            }));
        }
    });

    let player_for_c = Arc::clone(&music_player);
    siv.add_global_callback('c', move |_| player_for_c.lock().unwrap().play_pause());

    let player_for_prev = Arc::clone(&music_player);
    siv.add_global_callback(Event::Key(Key::Left), move |_| {
        let _ = player_for_prev.lock().unwrap().previous();
    });

    let player_for_next = Arc::clone(&music_player);
    siv.add_global_callback(Event::Key(Key::Right), move |_| {
        let mut mp = player_for_next.lock().unwrap();
        mp.skip();
    });

    siv.add_global_callback('q', |s| s.quit());
    siv.add_global_callback(event::Key::Esc, |s| s.select_menubar());
    siv.run();

    Ok(())
}
