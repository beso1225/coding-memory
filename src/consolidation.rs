//! Closed consolidation/review contracts and engine-owned note construction.
use crate::{
    consolidation_state::{Batch, Change},
    domain::{Evidence, hash},
    notes::{self, MemoryScope, Metadata, Note, Reference, Review, Status, Verification},
};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Existing {
    pub id: String,
    pub path: String,
    pub revision: String,
    pub markdown: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Create,
    Update,
    Supersede,
    Skip,
    Hold,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Draft {
    pub title: String,
    pub summary: String,
    pub scope: MemoryScope,
    pub conditions: Vec<String>,
    pub verification: Verification,
    pub evidence: Vec<Reference>,
    pub related: Vec<String>,
    pub body: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Operation {
    pub id: String,
    pub action: Action,
    pub candidates: Vec<String>,
    pub target: Option<String>,
    pub draft: Option<Draft>,
    pub dependencies: Vec<String>,
    pub rationale: String,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Proposal {
    pub schema_version: u32,
    pub role: String,
    pub operations: Vec<Operation>,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Decision {
    pub operation: String,
    pub decision: String,
    pub rationale: String,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Decisions {
    pub schema_version: u32,
    pub role: String,
    pub decisions: Vec<Decision>,
}
fn text(s: &str, max: usize) -> Result<()> {
    ensure!(
        !s.trim().is_empty() && s.len() <= max && !s.contains('\0'),
        "Invalid consolidation text"
    );
    Ok(())
}
pub fn validate(
    proposal: &Proposal,
    batch: &Batch,
    existing: &[Existing],
    evidence: &HashMap<String, Evidence>,
) -> Result<()> {
    ensure!(
        proposal.schema_version == 1
            && proposal.role == "consolidation"
            && !proposal.operations.is_empty()
            && proposal.operations.len() <= 20,
        "Invalid consolidation batch"
    );
    let candidates: HashMap<_, _> = batch
        .candidates
        .iter()
        .map(|c| (c.id.as_str(), &c.candidate))
        .collect();
    let notes: HashMap<_, _> = existing.iter().map(|n| (n.id.as_str(), n)).collect();
    let mut seen = HashSet::new();
    let mut assigned = HashSet::new();
    let mut targets = HashSet::new();
    for op in &proposal.operations {
        notes::identity(&op.id)?;
        text(&op.rationale, 2048)?;
        ensure!(
            op.dependencies.len() <= 20 && op.dependencies.iter().all(|d| seen.contains(d)),
            "Dependencies must name earlier operations"
        );
        ensure!(seen.insert(op.id.clone()), "Duplicate operation");
        ensure!(
            !op.candidates.is_empty() && op.candidates.len() <= 20,
            "Operation has no candidate"
        );
        let mut support = Vec::new();
        for id in &op.candidates {
            ensure!(assigned.insert(id.as_str()), "Candidate used twice");
            support.push(
                *candidates
                    .get(id.as_str())
                    .ok_or_else(|| anyhow::anyhow!("Foreign candidate"))?,
            );
        }
        let old = op
            .target
            .as_ref()
            .map(|id| {
                notes
                    .get(id.as_str())
                    .ok_or_else(|| anyhow::anyhow!("Target not supplied"))
            })
            .transpose()?;
        match op.action {
            Action::Create => ensure!(
                old.is_none() && op.draft.is_some(),
                "Create needs a fresh draft"
            ),
            Action::Update | Action::Supersede => {
                ensure!(
                    old.is_some() && op.draft.is_some(),
                    "Mutation needs target and draft"
                );
                ensure!(
                    targets.insert(op.target.as_ref().unwrap()),
                    "Target changed twice"
                );
            }
            Action::Skip => ensure!(
                old.is_some() && op.draft.is_none(),
                "Skip requires supplied existing knowledge"
            ),
            Action::Hold => ensure!(op.draft.is_none(), "Hold cannot carry publication"),
        }
        if let Some(d) = &op.draft {
            text(&d.body, 32 * 1024)?;
            ensure!(
                !d.body.contains("[[") && !d.body.contains("]]"),
                "Engine renders supplied link identities"
            );
            ensure!(
                d.related.len() <= 32 && d.related.iter().all(|id| notes.contains_key(id.as_str())),
                "Link outside supplied notes"
            );
            if let MemoryScope::Project { project_id } = &d.scope {
                ensure!(project_id == &batch.project, "Foreign scope");
            } else {
                ensure!(
                    support.iter().all(|c| c.scope == MemoryScope::Global),
                    "Global scope requires global candidate support"
                );
            }
            if let Some(old) = old {
                let old = notes::parse(&old.markdown)?;
                ensure!(
                    old.metadata.scope == d.scope && old.metadata.status == Status::Active,
                    "Scope or historical identity cannot be reassigned"
                );
                if matches!(op.action, Action::Update) {
                    ensure!(
                        d.verification == old.metadata.verification,
                        "Minor updates must retain the existing verification category"
                    );
                    ensure!(!d.related.contains(&old.metadata.id), "Self-related update");
                    ensure!(
                        old.metadata
                            .conditions
                            .iter()
                            .all(|c| d.conditions.contains(c))
                            && old.metadata.evidence.iter().all(|r| d.evidence.contains(r)),
                        "Minor updates must preserve conditions and evidence"
                    );
                }
            }
            for c in support {
                // These categories describe different support; they are not interchangeable.
                ensure!(
                    d.verification == c.verification,
                    "Consolidation must preserve every claim's verification category; separate or hold mixed support"
                );
                ensure!(
                    c.conditions.iter().all(|c| d.conditions.contains(c))
                        && c.evidence.iter().all(|r| d.evidence.contains(r)),
                    "Support/conditions lost during consolidation"
                );
            }
            let mut observed = false;
            let mut stated = false;
            for r in &d.evidence {
                let b = evidence
                    .get(&r.bundle)
                    .ok_or_else(|| anyhow::anyhow!("Foreign evidence bundle"))?;
                let i = b
                    .items
                    .iter()
                    .find(|i| i["id"].as_str() == Some(&r.item))
                    .ok_or_else(|| anyhow::anyhow!("Invalid evidence locator"))?;
                observed |= matches!(
                    i["type"].as_str(),
                    Some(
                        "commandExecution"
                            | "mcpToolCall"
                            | "fileChange"
                            | "webSearch"
                            | "dynamicToolCall"
                    )
                ) && i["status"] == "completed";
                stated |= i["type"] == "userMessage";
            }
            ensure!(
                match d.verification {
                    Verification::Observed => observed,
                    Verification::UserStated => stated,
                    Verification::Unverified => false,
                },
                "Insufficient verification support"
            );
            // Validate all bounds through the authoritative note schema before review.
            render_draft(
                d,
                "validation-note",
                None,
                &Review {
                    decision: "accepted".into(),
                    backend: "validation".into(),
                    model: "validation".into(),
                    prompt_version: "review-v1".into(),
                    reviewed_at: 0,
                },
                0,
                existing,
            )?;
        }
    }
    ensure!(
        assigned.len() == candidates.len(),
        "Every candidate must receive an operation"
    );
    Ok(())
}
fn render_draft(
    d: &Draft,
    id: &str,
    old: Option<&Metadata>,
    review: &Review,
    now: i64,
    existing: &[Existing],
) -> Result<String> {
    let mut body = d.body.clone();
    for related in &d.related {
        let n = existing
            .iter()
            .find(|n| &n.id == related)
            .ok_or_else(|| anyhow::anyhow!("Missing related note"))?;
        let stem = n
            .path
            .strip_suffix(".md")
            .ok_or_else(|| anyhow::anyhow!("Invalid link path"))?;
        ensure!(
            !stem.contains(['[', ']', '|', '#', '\n', '\r']),
            "Unrenderable Wikilink path"
        );
        body.push_str(&format!("\n\nRelated: [[{stem}|{related}]]"));
    }
    notes::render(&Note {
        metadata: Metadata {
            schema_version: 1,
            id: id.into(),
            title: d.title.clone(),
            summary: d.summary.clone(),
            scope: d.scope.clone(),
            status: Status::Active,
            successor: None,
            conditions: d.conditions.clone(),
            verification: d.verification.clone(),
            evidence: d.evidence.clone(),
            review: review.clone(),
            related: d.related.clone(),
            created_at: old.map_or(now, |m| m.created_at),
            updated_at: now,
        },
        body,
    })
}
pub fn plan(
    proposal: &Proposal,
    decisions: &Decisions,
    batch: &Batch,
    existing: &[Existing],
    review: &Review,
    now: i64,
) -> Result<(Vec<Change>, Vec<String>)> {
    ensure!(
        decisions.schema_version == 1
            && decisions.role == "review"
            && decisions.decisions.len() == proposal.operations.len(),
        "Incomplete review"
    );
    let mut accepted = HashSet::new();
    let mut ids = HashSet::new();
    for d in &decisions.decisions {
        text(&d.rationale, 2048)?;
        ensure!(
            proposal.operations.iter().any(|op| op.id == d.operation)
                && ids.insert(&d.operation)
                && ["accept", "reject", "hold"].contains(&d.decision.as_str()),
            "Invalid review operation"
        );
        if d.decision == "accept" {
            accepted.insert(d.operation.clone());
        }
    }
    loop {
        let mut remove = Vec::new();
        for op in &proposal.operations {
            if accepted.contains(&op.id)
                && (matches!(op.action, Action::Hold)
                    || op.dependencies.iter().any(|d| !accepted.contains(d))
                    || op.draft.as_ref().is_some_and(|d| {
                        d.related.iter().any(|id| {
                            proposal.operations.iter().any(|other| {
                                other.target.as_ref() == Some(id) && !accepted.contains(&other.id)
                            })
                        })
                    }))
            {
                remove.push(op.id.clone());
            }
        }
        if remove.is_empty() {
            break;
        }
        for id in remove {
            accepted.remove(&id);
        }
    }
    let mut changes = Vec::new();
    let mut consumed = Vec::new();
    for op in &proposal.operations {
        if !accepted.contains(&op.id) {
            continue;
        }
        consumed.extend(op.candidates.clone());
        if matches!(op.action, Action::Skip) {
            continue;
        }
        let d = op
            .draft
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Accepted mutation lacks draft"))?;
        let old = op
            .target
            .as_ref()
            .and_then(|id| existing.iter().find(|n| &n.id == id));
        let old_note = old.map(|n| notes::parse(&n.markdown)).transpose()?;
        let id = if matches!(op.action, Action::Update) {
            op.target.clone().unwrap()
        } else {
            format!(
                "memory-{}",
                hash(format!("{}:{}", batch.id, op.id).as_bytes())
            )
        };
        let content = render_draft(
            d,
            &id,
            if matches!(op.action, Action::Update) {
                old_note.as_ref().map(|n| &n.metadata)
            } else {
                None
            },
            review,
            now,
            existing,
        )?;
        changes.push(Change {
            note_id: id.clone(),
            path: if matches!(op.action, Action::Update) {
                old.unwrap().path.clone()
            } else {
                format!("{id}.md")
            },
            before: if matches!(op.action, Action::Update) {
                Some(old.unwrap().revision.clone())
            } else {
                None
            },
            after: hash(content.as_bytes()),
            content,
        });
        if matches!(op.action, Action::Supersede) {
            let old = old.unwrap();
            let mut note = old_note.unwrap();
            note.metadata.status = Status::Superseded;
            note.metadata.successor = Some(id.clone());
            note.metadata.updated_at = now;
            note.metadata.review = review.clone();
            note.body.push_str(&format!("\n\nSuperseded by: [[{id}]]"));
            let content = notes::render(&note)?;
            changes.push(Change {
                note_id: old.id.clone(),
                path: old.path.clone(),
                before: Some(old.revision.clone()),
                after: hash(content.as_bytes()),
                content,
            });
        }
    }
    // Read dependencies (skip and related notes) also need durable revision guards.
    for op in &proposal.operations {
        if !accepted.contains(&op.id) {
            continue;
        }
        let mut guard_ids = op
            .draft
            .as_ref()
            .map(|d| d.related.clone())
            .unwrap_or_default();
        if matches!(op.action, Action::Skip) {
            guard_ids.push(op.target.clone().unwrap());
        }
        for id in guard_ids {
            if changes.iter().any(|c| c.note_id == id) {
                continue;
            }
            let note = existing
                .iter()
                .find(|n| n.id == id)
                .ok_or_else(|| anyhow::anyhow!("Missing read dependency"))?;
            changes.push(Change {
                note_id: note.id.clone(),
                path: note.path.clone(),
                before: Some(note.revision.clone()),
                after: note.revision.clone(),
                content: note.markdown.clone(),
            });
        }
    }
    Ok((changes, consumed))
}
fn object(properties: Value) -> Value {
    let required: Vec<_> = properties.as_object().unwrap().keys().cloned().collect();
    json!({"type":"object","additionalProperties":false,"required":required,"properties":properties})
}
fn string() -> Value {
    json!({"type":"string"})
}
fn array(item: Value) -> Value {
    json!({"type":"array","maxItems":32,"items":item})
}
fn supplied(ids: Vec<String>) -> Value {
    if ids.is_empty() {
        json!({"type":"null"})
    } else {
        json!({"anyOf":[{"type":"string","enum":ids},{"type":"null"}]})
    }
}
pub fn proposal_schema(batch: &Batch, existing: &[Existing]) -> Value {
    let scope = object(
        json!({"kind":{"type":"string","enum":["project"]},"project_id":{"type":"string","enum":[batch.project]}}),
    );
    let draft = object(
        json!({"title":string(),"summary":string(),"body":string(),"scope":{"anyOf":[scope,object(json!({"kind":{"type":"string","enum":["global"]}}))]},"conditions":array(string()),"verification":{"type":"string","enum":["observed","user_stated","unverified"]},"related":array(string()),"evidence":array(object(json!({"bundle":string(),"item":string()})))}),
    );
    let op = object(
        json!({"id":string(),"action":{"type":"string","enum":["create","update","supersede","skip","hold"]},"candidates":array(json!({"type":"string","enum":batch.candidates.iter().map(|c|&c.id).collect::<Vec<_>>() })),"target":supplied(existing.iter().map(|n|n.id.clone()).collect()),"draft":{"anyOf":[draft,{"type":"null"}]},"dependencies":array(string()),"rationale":string()}),
    );
    object(
        json!({"schema_version":{"type":"integer","enum":[1]},"role":{"type":"string","enum":["consolidation"]},"operations":array(op)}),
    )
}
pub fn review_schema(proposal: &Proposal) -> Value {
    object(
        json!({"schema_version":{"type":"integer","enum":[1]},"role":{"type":"string","enum":["review"]},"decisions":array(object(json!({"operation":{"type":"string","enum":proposal.operations.iter().map(|o|&o.id).collect::<Vec<_>>()},"decision":{"type":"string","enum":["accept","reject","hold"]},"rationale":string()})))}),
    )
}
