//! Content-free sizing reports and an explicitly read-only held-batch preview.
use crate::{consolidation_state::Batch, consolidation_worker, search::Search, state::State};
use anyhow::{Result, ensure};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::{collections::HashSet, path::Path};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputBudget {
    pub consolidation_bytes: usize,
    pub consolidation_target: usize,
    pub review_max_bytes: usize,
    pub input_limit: usize,
    pub proposal_limit: usize,
}
impl InputBudget {
    pub(crate) fn fits(&self) -> bool {
        self.consolidation_bytes <= self.consolidation_target
            && self.review_max_bytes <= self.input_limit
    }
}
/// Bounded preparation reason. `code` is the single source of the serialized
/// strings: serde, `Display`, and hold classification all derive from it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    TooLarge,
    Unavailable,
    ExternalPolicy,
}
impl Reason {
    const ALL: [Self; 3] = [Self::TooLarge, Self::Unavailable, Self::ExternalPolicy];
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::TooLarge => "held_context_too_large",
            Self::Unavailable => "held_context_unavailable",
            Self::ExternalPolicy => "held_external_policy",
        }
    }
    fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|reason| reason.code() == code)
    }
}
impl Serialize for Reason {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(self.code())
    }
}
impl<'de> Deserialize<'de> for Reason {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let code = String::deserialize(deserializer)?;
        Self::from_code(&code)
            .ok_or_else(|| serde::de::Error::custom(format!("unknown reason code `{code}`")))
    }
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Detail {
    SourceMissing,
    SourceUnavailable,
    SourceReadLimit,
    SourceHashMismatch,
    SourceInvalidHash,
    SourceInvalidFormat,
    SourceUnsupported,
    SourceLocatorMissing,
    SourceLocatorAmbiguous,
    ExternalPolicy,
    ContextUnavailable,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextDiagnostic {
    pub candidate_ids: Vec<String>,
    pub candidate_count: usize,
    pub related_note_count: Option<usize>,
    pub referenced_item_count: Option<usize>,
    pub evidence_bundle_count: Option<usize>,
    pub budget: Option<InputBudget>,
    pub reason: Option<Reason>,
    pub detail: Option<Detail>,
}
impl ContextDiagnostic {
    pub(crate) fn new(batch: &Batch) -> Self {
        Self {
            candidate_ids: batch.candidates.iter().map(|c| c.id.clone()).collect(),
            candidate_count: batch.candidates.len(),
            related_note_count: None,
            referenced_item_count: None,
            evidence_bundle_count: None,
            budget: None,
            reason: None,
            detail: None,
        }
    }
    pub(crate) fn failure(mut self, reason: Reason, detail: Detail) -> anyhow::Error {
        self.reason = Some(reason);
        self.detail = Some(detail);
        DiagnosticError(self).into()
    }
}
#[derive(Debug)]
pub(crate) struct DiagnosticError(pub ContextDiagnostic);
impl std::fmt::Display for DiagnosticError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0.reason.unwrap_or(Reason::Unavailable).code())
    }
}
impl std::error::Error for DiagnosticError {}
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PartitionDiagnostic {
    pub selected: Option<ContextDiagnostic>,
    pub oversized: Vec<ContextDiagnostic>,
    pub deferred_count: usize,
    pub error: Option<ContextDiagnostic>,
}
impl PartitionDiagnostic {
    pub(crate) fn held_candidate(&self, id: &str) -> Option<Self> {
        self.oversized
            .iter()
            .find(|d| d.candidate_ids.iter().any(|c| c == id))
            .map(|d| Self {
                oversized: vec![d.clone()],
                ..Self::default()
            })
    }
}
#[derive(Serialize)]
pub struct BatchPreview {
    pub batch: i64,
    pub partitions: Vec<PartitionDiagnostic>,
    pub error: Option<ContextDiagnostic>,
}
#[derive(Serialize)]
pub struct Preflight {
    pub schema_version: u32,
    pub project: String,
    pub assumes_external: bool,
    pub snapshot_only: bool,
    pub invalid_note_count: usize,
    pub batches: Vec<BatchPreview>,
}

/// This function has no Backend and never reserves, retries, or publishes work.
/// Each requested batch models its own hypothetical retry, numbering partitions
/// from the same next batch ID. Future publications may change neighbor context,
/// so later partitions are estimates.
pub fn preflight(
    state: &State,
    root: &Path,
    vault: &Path,
    project: &str,
    batches: &[i64],
    external: bool,
) -> Result<Preflight> {
    ensure!(
        (1..=20).contains(&batches.len()) && batches.iter().all(|id| *id > 0),
        "Specify 1..20 positive held batch IDs"
    );
    ensure!(
        batches.iter().collect::<HashSet<_>>().len() == batches.len(),
        "Duplicate batch IDs"
    );
    state.project_epoch(project)?;
    // Validate every requested batch before doing potentially expensive source reads.
    let originals = batches
        .iter()
        .map(|id| state.preflight_batch(project, *id))
        .collect::<Result<Vec<_>>>()?;
    let search = Search::temporary(root, vault)?;
    let invalid_note_count = search.rebuild()?.invalid.len();
    let base_id = state.next_batch_id()?;
    let mut reports = Vec::new();
    for mut batch in originals {
        let original_id = batch.id;
        // Each requested batch is modeled as its own hypothetical retry, so every
        // batch starts from the same next-batch base and only advances across its
        // own partitions. Otherwise later batches would inherit synthetic IDs that a
        // standalone retry would never allocate, skewing the serialized sizes.
        let mut next_id = base_id;
        let mut partitions = Vec::new();
        let mut error = None;
        for c in &mut batch.candidates {
            c.state = "ready".into();
        }
        while !batch.candidates.is_empty() {
            batch.id = next_id;
            match consolidation_worker::preview_partition(state, root, &search, &batch, external) {
                Ok(plan) => {
                    let mut removed = HashSet::new();
                    if let Some(selected) = &plan.selected {
                        removed.extend(selected.candidate_ids.iter().cloned());
                    }
                    for held in &plan.oversized {
                        removed.extend(held.candidate_ids.iter().cloned());
                    }
                    ensure!(!removed.is_empty(), "Size preview made no progress");
                    let allocated = if plan.selected.is_some() {
                        1 + plan.oversized.len()
                    } else {
                        plan.oversized.len().max(1)
                    };
                    next_id = next_id
                        .checked_add(allocated as i64)
                        .ok_or_else(|| anyhow::anyhow!("Batch ID overflow"))?;
                    batch.candidates.retain(|c| !removed.contains(&c.id));
                    partitions.push(plan);
                }
                Err(failure) => {
                    error = Some(
                        failure
                            .downcast_ref::<DiagnosticError>()
                            .map(|e| e.0.clone())
                            .unwrap_or_else(|| {
                                let mut d = ContextDiagnostic::new(&batch);
                                d.reason = Some(Reason::Unavailable);
                                d.detail = Some(Detail::ContextUnavailable);
                                d
                            }),
                    );
                    break;
                }
            }
        }
        reports.push(BatchPreview {
            batch: original_id,
            partitions,
            error,
        });
    }
    Ok(Preflight {
        schema_version: 1,
        project: project.into(),
        assumes_external: external,
        snapshot_only: true,
        invalid_note_count,
        batches: reports,
    })
}

#[cfg(test)]
mod tests {
    use super::Reason;
    #[test]
    fn reason_serialization_is_derived_from_its_code() -> anyhow::Result<()> {
        for reason in Reason::ALL {
            let encoded = serde_json::to_string(&reason)?;
            assert_eq!(encoded, format!("\"{}\"", reason.code()));
            assert_eq!(serde_json::from_str::<Reason>(&encoded)?, reason);
        }
        assert!(serde_json::from_str::<Reason>("\"unknown\"").is_err());
        Ok(())
    }
}
