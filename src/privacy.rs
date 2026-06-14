/// Privacy and security utilities for Pluriview
use std::collections::HashSet;
use once_cell::sync::Lazy;

/// A list of process names that should never be captured for privacy reasons.
/// Users can eventually customize this in settings.
pub static BLACKLISTED_PROCESSES: Lazy<HashSet<&'static str>> = Lazy::new(|| {
    let mut m = HashSet::new();
    m.insert("1Password.exe");
    m.insert("Bitwarden.exe");
    m.insert("KeePassXC.exe");
    m.insert("LastPass.exe");
    m.insert("Dashlane.exe");
    m.insert("Enpass.exe");
    // Add common sensitive apps
    m.insert("Signal.exe");
    m.insert("Telegram.exe");
    m.insert("WhatsApp.exe");
    m
});

/// Redact a window title for safe logging.
/// In release builds, this returns a shortened/masked version of the title.
pub fn redact_title(title: &str) -> String {
    if cfg!(debug_assertions) {
        title.to_string()
    } else {
        let chars: Vec<char> = title.chars().collect();
        if chars.len() <= 4 {
            "***".to_string()
        } else {
            let head: String = chars[..2].iter().collect();
            let tail: String = chars[chars.len() - 2..].iter().collect();
            format!("{}***{}", head, tail)
        }
    }
}

/// Check if a window should be ignored based on its process name or title.
pub fn is_sensitive_window(exe_name: &str, title: &str) -> bool {
    // Check process blacklist
    if BLACKLISTED_PROCESSES.contains(exe_name) {
        return true;
    }

    // Check for sensitive keywords in title
    let sensitive_keywords = ["password", "private", "incognito", "secret"];
    let lower_title = title.to_lowercase();
    
    sensitive_keywords.iter().any(|&k| lower_title.contains(k))
}
