//! Various utilities used by rsmus.

pub fn format_time(secs: usize) -> String {
    let minutes = secs / 60;
    let seconds = secs % 60;
    format!("{}:{:02}", minutes, seconds)
}

pub fn clean_title(title: &str) -> String {
    let patterns = ["(explicit)", "[explicit]", "(Explicit)", "[Explicit]", "Explicit", "- Explicit", "(Clean)", "[Clean]"];

    let mut cleaned = title.to_string();
    for pattern in &patterns {
        cleaned = cleaned.replace(pattern, "");
    }

    cleaned.trim().to_string()
}
