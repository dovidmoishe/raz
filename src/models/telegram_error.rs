/// Telegram rejection without the credential-bearing request URL.
#[derive(Debug)]
pub struct TelegramApiError {
    pub method: String,
    pub status: u16,
    pub description: String,
}

impl std::fmt::Display for TelegramApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "telegram {} failed ({}): {}",
            self.method, self.status, self.description
        )
    }
}

impl std::error::Error for TelegramApiError {}
