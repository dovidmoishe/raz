use serde::Serialize;

use super::{now_unix, Engine};

#[derive(Clone, Debug, Serialize)]
pub struct TurnEvent {
    pub ts: u64,
    pub event: String,
    pub job: String,
    pub engine: String,
    pub project: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ok: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit: Option<i32>,
}

impl TurnEvent {
    pub fn start(job: &str, engine: Engine, project: &str) -> Self {
        Self {
            ts: now_unix(),
            event: "start".into(),
            job: job.to_string(),
            engine: engine.as_str().to_string(),
            project: project.to_string(),
            ok: None,
            exit: None,
        }
    }

    pub fn end(job: &str, engine: Engine, project: &str, ok: bool, exit: Option<i32>) -> Self {
        Self {
            ts: now_unix(),
            event: "end".into(),
            job: job.to_string(),
            engine: engine.as_str().to_string(),
            project: project.to_string(),
            ok: Some(ok),
            exit,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::TurnEvent;
    use crate::models::Engine;

    #[test]
    fn json_has_no_prompt_field() {
        let start = TurnEvent::start("j_ab3", Engine::Claude, "shop");
        let line = serde_json::to_string(&start).unwrap();
        assert!(line.contains("\"event\":\"start\""));
        assert!(!line.contains("prompt"));
        assert!(!line.contains("\"ok\""));
        let end = TurnEvent::end("j_ab3", Engine::Codex, "shop", true, Some(0));
        let line = serde_json::to_string(&end).unwrap();
        assert!(line.contains("\"ok\":true"));
        assert!(line.contains("\"exit\":0"));
        assert!(!line.contains("prompt"));
    }
}
