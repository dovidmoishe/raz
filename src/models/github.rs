use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct GithubStatus {
    pub ready: bool,
    pub user: Option<String>,
}
