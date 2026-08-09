# rsmus

A music player written in Rust for the modern CLI era.

## Features

* Basic music library - shows your artists, albums, and the tracks associated with them in a tree-like view.
  * Artists and albums can be individually collapsed to allow focusing on specific artists or albums. **NOTE**: These states do not persist across app launches.
* Basic playback support - Play/pause, skip/rewind, and toggling of repeat state.
* Metdata dialog - shows the track metadata of the currently playing track.

## Keyboard Shortcuts

| Function                | Shortcut         |
| -------------           | -------------    |
| Navigate Library        | `Up` / `Down`    |
| Collapse Artist / Album | `Space`          |
| Play / Pause            | `c`              |
| Previous / Next         | `Left` / `Right` |
| Toggle Repeat State     | `r`              |
| View Track Metadata     | `m`              |
| Quit                    | `q`              |
