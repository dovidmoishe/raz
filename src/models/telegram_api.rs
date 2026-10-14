use serde::Deserialize;

#[derive(Debug, Deserialize, Clone)]
pub struct TelegramUpdate {
    pub update_id: i64,
    pub message: Option<Message>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Message {
    pub chat: Chat,
    pub from: Option<TelegramUser>,
    pub text: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Chat {
    pub id: i64,
}

#[derive(Debug, Deserialize, Clone)]
pub struct TelegramUser {
    pub id: i64,
    pub username: Option<String>,
}
