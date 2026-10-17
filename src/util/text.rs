pub const TELEGRAM_MAX: usize = 4096;

pub fn clip_chars(text: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    if text.chars().count() <= max {
        return text.to_string();
    }
    if max == 1 {
        return "…".into();
    }
    let mut out: String = text.chars().take(max - 1).collect();
    out.push('…');
    out
}

pub fn clip_telegram(text: &str) -> String {
    clip_chars(text, TELEGRAM_MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clips_at_limit() {
        assert_eq!(clip_chars("abc", 3), "abc");
        assert_eq!(clip_chars("abcd", 3), "ab…");
        assert_eq!(clip_telegram("ok"), "ok");
        let long = "x".repeat(TELEGRAM_MAX + 8);
        let clipped = clip_telegram(&long);
        assert_eq!(clipped.chars().count(), TELEGRAM_MAX);
        assert!(clipped.ends_with('…'));
    }
}
