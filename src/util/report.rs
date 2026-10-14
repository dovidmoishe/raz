use std::time::{SystemTime, UNIX_EPOCH};

fn stamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn info(scope: &str, message: impl AsRef<str>) {
    eprintln!("info\t{}\t{scope}\t{}", stamp(), message.as_ref());
}

pub fn warn(scope: &str, message: impl AsRef<str>) {
    eprintln!("warn\t{}\t{scope}\t{}", stamp(), message.as_ref());
}

pub fn error(scope: &str, message: impl AsRef<str>) {
    eprintln!("error\t{}\t{scope}\t{}", stamp(), message.as_ref());
}
