use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TelegramState {
    pub allowed_user_ids: Vec<i64>,
    pub paired: bool,
    #[serde(default)]
    pub pairing_code: Option<String>,
    #[serde(default)]
    pub pairing_expires_unix: Option<u64>,
    #[serde(default)]
    pub last_update_id: i64,
}
