use std::io::Write;

pub fn format_time(secs: usize) -> String {
    let minutes = secs / 60;
    let seconds = secs % 60;
    format!("{}:{:02}", minutes, seconds)
}

/// Strip various explicit/clean labeling from song titles, in order
/// to allow for cleaner song titles, as some storefronts include
/// the "Explicit" or "Clean" labeling in the music metadata. This
/// allows for cleaner song titles without the user having to go in
/// and manually update their music metadata to remove the "Explicit"
/// or "Clean" labeling.
pub fn clean_title(title: &str) -> String {
    let patterns = ["(explicit)", "[explicit]", "(Explicit)", "[Explicit]", "Explicit", "- Explicit", "(Clean)", "[Clean]"];

    let mut cleaned = title.to_string();
    for pattern in &patterns {
        cleaned = cleaned.replace(pattern, "");
    }

    cleaned.trim().to_string()
}

/// Sanitizes input to remove control characters that could break out of terminal escape sequences.
fn sanitize_terminal_string(input: &str) -> String {
    input.chars().filter(|c| !c.is_control() || *c == ' ').collect()
}

/// Updates the terminal title to reflect current playback status safely.
pub fn update_terminal_title(title: &str, artist: &str, album: &str, year: &str, is_paused: bool) {
    let indicator = if is_paused { "⏸" } else { "▶" };

    let safe_title = sanitize_terminal_string(title);
    let safe_artist = sanitize_terminal_string(artist);
    let safe_album = sanitize_terminal_string(album);
    let safe_year = sanitize_terminal_string(year);

    let formatted_title = format!("{} {} by {} ({} - {})", indicator, safe_title, safe_artist, safe_album, safe_year);
    print!("\x1b]0;{}\x07", formatted_title);
    let _ = std::io::stdout().flush();
}
