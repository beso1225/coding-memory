//! Versioned authoritative Markdown note format.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

pub const MAX_NOTE_BYTES: usize = 64 * 1024;
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum MemoryScope {
    Project { project_id: String },
    Global,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Active,
    Superseded,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verification {
    Observed,
    UserStated,
    Unverified,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reference {
    pub bundle: String,
    pub item: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Review {
    pub decision: String,
    pub backend: String,
    pub model: String,
    pub prompt_version: String,
    pub reviewed_at: i64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Metadata {
    pub schema_version: u32,
    pub id: String,
    pub title: String,
    pub summary: String,
    pub scope: MemoryScope,
    pub status: Status,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub successor: Option<String>,
    pub conditions: Vec<String>,
    pub verification: Verification,
    pub evidence: Vec<Reference>,
    pub review: Review,
    pub related: Vec<String>,
    pub created_at: i64,
    pub updated_at: i64,
}
#[derive(Debug, Clone)]
pub struct Note {
    pub metadata: Metadata,
    pub body: String,
}
pub fn identity(value: &str) -> Result<()> {
    ensure!(
        !value.is_empty()
            && value.len() <= 128
            && value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
        "Invalid memory/project identity"
    );
    Ok(())
}
fn text(value: &str, max: usize) -> Result<()> {
    ensure!(
        !value.trim().is_empty() && value.len() <= max && !value.contains('\0'),
        "Invalid or oversized note text"
    );
    Ok(())
}
impl Metadata {
    pub fn validate(&self) -> Result<()> {
        ensure!(self.schema_version == 1, "Unsupported note schema");
        identity(&self.id)?;
        if let MemoryScope::Project { project_id } = &self.scope {
            identity(project_id)?;
        }
        text(&self.title, 256)?;
        text(&self.summary, 512)?;
        ensure!(
            self.conditions.len() <= 16,
            "Too many applicability conditions"
        );
        for c in &self.conditions {
            text(c, 256)?;
        }
        ensure!(
            !self.evidence.is_empty() && self.evidence.len() <= 16,
            "Missing or excessive evidence references"
        );
        for reference in &self.evidence {
            ensure!(
                reference.bundle.len() == 64
                    && reference
                        .bundle
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
                "Invalid evidence digest"
            );
            text(&reference.item, 256)?;
        }
        ensure!(
            self.review.decision == "accepted",
            "Only accepted memory notes may be indexed"
        );
        for field in [
            &self.review.backend,
            &self.review.model,
            &self.review.prompt_version,
        ] {
            text(field, 256)?;
        }
        ensure!(
            self.created_at >= 0
                && self.updated_at >= self.created_at
                && self.review.reviewed_at >= 0,
            "Invalid note timestamps"
        );
        match (&self.status, &self.successor) {
            (Status::Active, None) => {}
            (Status::Superseded, Some(id)) if id != &self.id => identity(id)?,
            _ => anyhow::bail!("Invalid supersession metadata"),
        }
        ensure!(self.related.len() <= 32, "Too many related notes");
        for id in &self.related {
            identity(id)?;
            ensure!(id != &self.id, "Self-related memory");
        }
        Ok(())
    }
}
pub fn parse(markdown: &str) -> Result<Note> {
    ensure!(markdown.len() <= MAX_NOTE_BYTES, "Note exceeds size limit");
    let normalized = markdown.replace("\r\n", "\n");
    let rest = normalized
        .strip_prefix("---\n")
        .ok_or_else(|| anyhow::anyhow!("Missing YAML frontmatter"))?;
    let (yaml, body) = rest
        .split_once("\n---\n")
        .ok_or_else(|| anyhow::anyhow!("Unclosed frontmatter"))?;
    ensure!(yaml.len() <= 16 * 1024, "Frontmatter exceeds size limit");
    let metadata: Metadata = serde_yaml_ng::from_str(yaml)?;
    metadata.validate()?;
    text(body, MAX_NOTE_BYTES)?;
    Ok(Note {
        metadata,
        body: body.into(),
    })
}
pub fn render(note: &Note) -> Result<String> {
    note.metadata.validate()?;
    let markdown = format!(
        "---\n{}---\n{}",
        serde_yaml_ng::to_string(&note.metadata)?,
        note.body
    );
    parse(&markdown)?;
    Ok(markdown)
}
