use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunKey {
    pub store: String,
    pub thread: String,
    pub turn: String,
}

impl RunKey {
    pub fn id(&self) -> String {
        hash(&serde_json::to_vec(self).expect("serializable identity"))
    }
}

pub fn hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeAudit {
    pub binary_sha256: String,
    pub reader_version: String,
    pub fixture: String,
    pub validation: String,
    pub checked_at: Option<i64>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Evidence {
    pub schema_version: u32,
    pub run: RunKey,
    pub project: String,
    pub runtime_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture_runtime: Option<RuntimeAudit>,
    pub outcome: String,
    pub completed_at: i64,
    pub started_at: i64,
    pub cwd: Option<String>,
    pub terminal_error: Option<serde_json::Value>,
    pub items: Vec<serde_json::Value>,
    pub coverage: Vec<String>,
}
