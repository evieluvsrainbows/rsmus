mod player;
mod utils;

use clap::Parser;
use cursive::{
    event::{self, Event, Key},
    menu,
    theme::Theme,
    view::Resizable,
    views::{Dialog, NamedView, TextView},
};
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
    let mut track_metadata_list = Vec::new();
    let mut paths_to_append = Vec::new();

    let tag_parser = audiotags::Tag::default();

    let extract_metadata = |filepath: &PathBuf| -> TrackMetadata {
        let fallback_name = filepath.file_name().and_then(|n| n.to_str()).unwrap_or("Unknown File").to_string();
        let duration = if let Ok(file) = File::open(filepath) {
            if let Ok(source) = Decoder::try_from(file) {
                source.total_duration().unwrap_or(Duration::from_secs(0))
            } else {
                Duration::from_secs(0)
            }
        } else {
            Duration::from_secs(0)
        };

        if let Ok(tag) = tag_parser.read_from_path(filepath) {
            let raw_title = tag.title().unwrap_or(&fallback_name);
            TrackMetadata {
                title: utils::clean_title(raw_title),
                artist: tag.artist().unwrap_or("Unknown Artist").to_string(),
                album: tag.album_title().unwrap_or("Unknown Album").to_string(),
                year: tag.year().unwrap_or(0000).to_string(),
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

    if std::fs::metadata(&input)?.is_dir() {
        let mut entries: Vec<_> = std::fs::read_dir(&input)?
            .filter_map(Result::ok)
            .filter(|f| {
                if let Ok(meta) = f.metadata() {
                    if meta.is_dir() {
                        return false;
                    }
                }
                let path_str = f.path().to_string_lossy().to_lowercase();
                !path_str.ends_with(".jpg") && !path_str.ends_with(".png")
            })
            .collect();

        let has_music_file = entries.iter().any(|f| {
            let path_str = f.path().to_string_lossy().to_lowercase();
            path_str.ends_with(".flac") || path_str.ends_with(".m4a") || path_str.ends_with(".mp3") || path_str.ends_with(".wav")
        });

        if !has_music_file {
            return Err(format!("No music files found in directory: {}", input).into());
        }

        entries.sort_by_key(|d| d.path());

        for f in entries {
            let filepath = f.path();
            track_metadata_list.push(extract_metadata(&filepath));
            paths_to_append.push(filepath);
        }
    } else {
        let filepath = PathBuf::from(&input);
        let path_str = filepath.to_string_lossy().to_lowercase();
        if !path_str.ends_with(".flac") && !path_str.ends_with(".mp3") && !path_str.ends_with(".m4a") && !path_str.ends_with(".wav") {
            return Err(format!("Specified file is not a supported music file: {}", input).into());
        }
        track_metadata_list.push(extract_metadata(&filepath));
        paths_to_append.push(filepath);
    }

    let music_player = Arc::new(Mutex::new(MusicPlayer::new(track_metadata_list, paths_to_append)?));
    {
        let mut mp = music_player.lock().unwrap();
        for filepath in mp.paths.clone() {
            let file = File::open(&filepath)?;
            let source = Decoder::try_from(file)?;
            mp.player.append(source);
        }
        mp.track_start_time = Instant::now();
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
        let mut previous_index = 0;
        loop {
            thread::sleep(Duration::from_millis(200));

            let mut mp = player_for_thread.lock().unwrap();
            let current_idx = mp.current_index;
            let (current_sec, total_sec) = mp.get_current_progress();

            if current_sec >= total_sec && total_sec > 0 && current_idx < mp.tracks.len() - 1 {
                mp.advance_track();
            } else if !mp.player.empty() && current_idx != previous_index {
                previous_index = current_idx;
            }

            let track_text = format!("{}", mp.current_track_info());
            let (current, total) = mp.get_current_progress();
            let time_text = format!("{} / {}", utils::format_time(current), utils::format_time(total));
            let indicator_text = if mp.is_paused { "⏸ " } else { "▶ " };

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
