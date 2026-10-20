use serde::{Deserialize, Serialize};

/// Optional onboarding choices. Readiness is checked against the actual tools.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SetupProgress {
    pub github_skipped: bool,
    pub service_skipped: bool,
    pub service_configured: bool,
}
