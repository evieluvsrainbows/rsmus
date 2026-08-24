use crate::player::MusicPlayer;
use cursive::{
    Cursive,
    views::{Dialog, TextView},
};
use std::sync::{Arc, Mutex};

/// Creates a dialog that shows the metadata of the currently
/// playing track.
pub(crate) fn show_metadata(siv: &mut Cursive, music_player: Arc<Mutex<MusicPlayer>>) {
    let Ok(mp) = music_player.lock() else { return };
    let Some(track) = mp.queue.get(mp.current_index) else { return };
    let meta_text = format!("{}\nPath:          {}", track.metadata, track.path.display());
    let dialog = Dialog::around(TextView::new(meta_text)).title(format!("Metadata for {}", track.metadata.title)).button("Close", |s| {
        s.pop_layer();
    });
    siv.add_layer(dialog);
}
