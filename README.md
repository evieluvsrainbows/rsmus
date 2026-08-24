# rsmus

A music player written in Rust for the modern CLI era.

## Getting started

To get started with rsmus, follow the following steps.

1. Download the repository (either as a zip file or cloning via git)
2. Navigate to the location where you downloaded, or cloned, rsmus.
3. Run `cargo run --release -- --scan \<directory>` to build rsmus and scan your music directory into its database.
4. Run `cargo run --release` without arguments to launch rsmus.

If desired, you can combine steps 3 and 4 into a single step for a faster initial setup and app launch:

```bash
cargo run --release -- --scan \<directory> && cargo run --release
```

For future app launches, if there is no need to scan additional music into the database, only `cargo run --release` is needed.

**NOTE**: rsmus *might* be added to crates.io later on as a directly installable binary when it is determined to be stable
enough, but it is not guaranteed that it will be added.

## Features
The following is a list of features currently offered by rsmus. Additional features are planned and will be added later on.

* Basic music library - shows your artists, albums, and the tracks associated with them in a tree-like view.
  * Artists and albums can be individually collapsed to allow focusing on specific artists or albums. **NOTE**: These states do not yet persist across app launches.
* Basic playback support - Play/pause, seeking, skip/rewind, and repeat.
* Basic persistence - the last played track and its track progress persists across app launches.
* Metdata dialog - shows the track metadata of the currently playing track.

## Keyboard shortcuts
The following table contains a list of keybindings used by rsmus for its various features. 

**NOTE**: These keyboard shortcuts are currently hardcoded and cannot yet be customized or rebound.

| Function                | Shortcut         |
| -------------           | -------------    |
| Navigate Library        | `Up` / `Down`    |
| Collapse Artist / Album | `Space`          |
| Play / Pause            | `c`              |
| Previous / Next         | `Left` / `Right` |
| Rewind (10s)            | `b`              |
| Forward (10s)           | `n`              |
| Toggle Repeat State     | `r`              |
| View Track Metadata     | `m`              |
| Quit                    | `q`              |

## Licence

This project is licenced under the terms of the Apache Licence; please see the [LICENCE](LICENCE) file for the full
terms of the Apache Licence as they govern how the rsmus project may be used, redistributed, or modified.
