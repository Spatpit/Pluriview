/// Privacy and security utilities for Pluriview
/// A list of process names that should never be captured for privacy reasons.
/// Users can eventually customize this in settings.
pub const BLACKLISTED_PROCESSES: &[&str] = &[
    "1Password.exe",
    "Bitwarden.exe",
    "KeePassXC.exe",
    "LastPass.exe",
    "Dashlane.exe",
    "Enpass.exe",
    "Signal.exe",
    "Telegram.exe",
    "WhatsApp.exe",
];

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
    if BLACKLISTED_PROCESSES
        .iter()
        .any(|blocked| exe_name.eq_ignore_ascii_case(blocked))
    {
        return true;
    }

    // Check for sensitive keywords in title
    let sensitive_keywords = ["password", "private", "incognito", "secret"];
    let lower_title = title.to_lowercase();

    sensitive_keywords.iter().any(|&k| lower_title.contains(k))
}

#[cfg(test)]
mod tests {
    use super::is_sensitive_window;

    #[test]
    fn process_blacklist_is_case_insensitive() {
        assert!(is_sensitive_window("bitwarden.exe", "Vault"));
        assert!(is_sensitive_window("KEEPASSXC.EXE", "Database"));
    }
}
