//! Various utilities used by rsmus.

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
