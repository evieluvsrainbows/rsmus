# rsmus

A terminal-based music player written in Rust.

## Getting started

To get started with rsmus, follow the following steps.

1. Download the repository (either as a zip file or cloning via git)
2. Navigate to the location where you downloaded, or cloned, rsmus.
3. Run `cargo run --release -- --scan \<directory>` to build rsmus and scan your music directory into its database.
4. Run `cargo run --release` without arguments to launch rsmus.

If desired, you can combine steps 3 and 4 into a single step for a faster initial setup and app launch:

```bash
cargo run --release -- --scan <directory> && cargo run --release
```

> [!NOTE]
> If you rename or remove any tracks / directories from the music folder where you imported music from, please ensure that you
> re-run the following command to ensure that the database is properly updated:
> ```bash
> cargo run --release -- --scan <directory>
> ```

For future app launches, if there is no need to scan additional music into the database and if there have been no updates to the
directory (or directories) you imported music from, only `cargo run --release` is needed.

> [!NOTE]
> rsmus *might* be added to crates.io later on as a directly installable binary when it is determined to be stable enough, 
> but it is not guaranteed that it will be added.

## Features
The following is a list of features currently offered by rsmus. Additional features are planned and will be added later
on.

* Basic music library - shows your artists, albums, and the tracks associated with them in a tree-like view.
  * Artists and albums can be individually collapsed or expanded to allow focusing on specific artists or albums.
* Basic playback support - Play/pause, seeking, skip/rewind, and repeat.
* Basic persistence - the last played track and its track progress persists across app launches.
* Metdata dialog - shows the track metadata of the currently playing track.

> [!NOTE]
> Collapsed/expanded states for artists or albums do not yet persist across app launches, but it is eventually
> planned to support this.

## Performance
Please note that performance of rsmus with large media libraries has not been tested. I have a small local media library
consisting of ~75 tracks, and observed performance has been fine from what has been observed with that number of tracks on
my M3 Max MacBook Pro when running rsmus in the [ghostty](https://ghostty.org) terminal emulator, but I have not tested it
on any other machines, hardware, or terminal emulators. Therefore, depending on your particular machine, terminal emulator,
and the size of your music library, your mileage may vary on how rsmus performs, so if any issues with performance arise, please
file an issue on GitHub and I will take a look, or if you know your way around Rust programming, feel free to submit a pull request
containing any potential performance optimisations.

As rsmus' main backend libraries (rusqlite, cursive) are synchronous rather than asynchronous, it is primarily single-threaded. A move
to async would be difficult without a migration to an async-compatible TUI library and SQLite wrapper like ratatui and sqlx, and that
kind of move is not yet planned.

## Keyboard shortcuts
The following table contains a list of keybindings used by rsmus for its various features. 

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

> [!NOTE]
> The above keyboard shortcuts are currently hardcoded and cannot yet be customized or rebound.

## Licence

This project is licenced under the terms of the Apache Licence; please see the [LICENCE](LICENSE) file for the full
terms of the Apache Licence as they govern how the rsmus project may be used, redistributed, or modified.
