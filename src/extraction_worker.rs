//! One bounded extraction attempt; SQLite transactions never span generation.
use crate::{
    domain::{Evidence, hash},
    evidence, extraction,
    extraction_state::ChunkPlanId,
    generation::{Backend, Request},
    ingestion,
    state::State,
};
use anyhow::{Result, ensure};
use serde_json::{Value, json};
use std::path::Path;

pub async fn extract_one(
    state: &mut State,
    root: &Path,
    project: &str,
    backend: &impl Backend,
) -> Result<bool> {
    // Validate the backend contract before claiming: otherwise a rejected provenance
    // object would strand the claim until its lease expires.
    let mut provenance = backend.provenance();
    ensure!(
        provenance.is_object(),
        "Backend provenance must be an object"
    );
    let Some(claim) = state.claim_extraction(project, ingestion::now(), 3600)? else {
        return Ok(false);
    };
    provenance["evidence_hash"] = json!(claim.evidence);
    provenance["external_transmission"] = json!(backend.external());
    if backend.external() && !state.external_allowed(project)? {
        state.fail_extraction(
            &claim,
            "held_external_policy",
            false,
            &provenance,
            ingestion::now(),
        )?;
        return Ok(true);
    }
    let input = (|| -> std::result::Result<Evidence, &'static str> {
        if claim.evidence.len() != 64
            || !claim
                .evidence
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err("held_invalid_evidence");
        }
        let path = root
            .join("evidence")
            .join(format!("{}.json", claim.evidence));
        provenance["source_limit"] = json!(evidence::MAX_SOURCE_BYTES);
        // One size reading classifies the hold, and the bounded read below records what was
        // actually parsed. An absent source and an unreadable one are different operator
        // problems, so they keep distinct reasons instead of sharing the corruption hold.
        let source_bytes = std::fs::metadata(&path)
            .map_err(|error| match error.kind() {
                std::io::ErrorKind::NotFound => "held_missing_evidence",
                _ => "held_unreadable_evidence",
            })?
            .len();
        provenance["source_bytes"] = json!(source_bytes);
        if source_bytes > evidence::MAX_SOURCE_BYTES as u64 {
            return Err("held_oversized_evidence_source");
        }
        let bytes =
            evidence::read_bounded(&path, evidence::MAX_SOURCE_BYTES).map_err(
                |error| match error.downcast_ref::<std::io::Error>().map(|e| e.kind()) {
                    Some(std::io::ErrorKind::NotFound) => "held_missing_evidence",
                    Some(_) => "held_unreadable_evidence",
                    None => "held_oversized_evidence_source",
                },
            )?;
        if hash(&bytes) != claim.evidence {
            return Err("held_invalid_evidence");
        }
        let bundle: Evidence =
            serde_json::from_slice(&bytes).map_err(|_| "held_invalid_evidence")?;
        if bundle.schema_version != 1
            || bundle.run != claim.key
            || bundle.project != claim.project
            || bundle.outcome != "completed"
        {
            return Err("held_invalid_evidence");
        }
        let mut ids = std::collections::HashSet::new();
        for item in &bundle.items {
            let id = item["id"].as_str().ok_or("held_invalid_evidence")?;
            if id.is_empty() || !ids.insert(id) {
                return Err("held_invalid_evidence");
            }
        }
        Ok(bundle)
    })();
    let bundle = match input {
        Ok(bundle) => bundle,
        Err(reason) => {
            state.fail_extraction(&claim, reason, false, &provenance, ingestion::now())?;
            return Ok(true);
        }
    };
    provenance["input_limit"] = json!(extraction::MAX_INPUT);
    provenance["item_count"] = json!(bundle.items.len());
    if bundle.items.is_empty() {
        // No native item can be cited, so the response schema would carry an empty item
        // enum and the call could only return an empty batch. Decide the empty success
        // locally instead of spending one.
        provenance["chunk_count"] = json!(0);
        provenance["completed_chunks"] = json!(0);
        state.finish_extraction(&claim, &[], &provenance, ingestion::now())?;
        return Ok(true);
    }
    let ranges = match extraction::chunks(&bundle, &claim.evidence) {
        Ok(ranges) => ranges,
        Err(reason) => {
            state.fail_extraction(&claim, reason, false, &provenance, ingestion::now())?;
            return Ok(true);
        }
    };
    provenance["chunk_count"] = json!(ranges.len());
    // A prefix recorded by an earlier attempt is reused only for the same evidence, prompt
    // version and partition; anything else restarts every chunk. Fences clear the record and
    // a fenced attempt cannot write it, so a takeover never resumes an offset it lost.
    let signature = plan_signature(&ranges);
    let plan = ChunkPlanId {
        evidence: &claim.evidence,
        prompt_version: extraction::PROMPT_VERSION,
        signature: &signature,
        chunks: ranges.len(),
    };
    let resumed = state.extraction_progress(&claim.key, &plan)?;
    let resumed_chunks = resumed
        .as_ref()
        .map_or(0, |progress| progress.completed_chunks);
    provenance["plan_hash"] = json!(signature);
    provenance["resumed_chunks"] = json!(resumed_chunks);
    provenance["completed_chunks"] = json!(resumed_chunks);
    let mut metadata = bundle;
    let items = std::mem::take(&mut metadata.items);
    let mut candidates = resumed.map_or_else(Vec::new, |progress| progress.candidates);
    // Processing failures below hold the job with a bounded reason and the provenance of
    // the chunks that completed. Fence and state-database errors propagate instead: an
    // attempt that lost its claim cannot write a hold, and lease recovery records
    // `generation_lease_expired` once its attempts are exhausted.
    for (index, range) in ranges.into_iter().enumerate() {
        if index < resumed_chunks {
            continue;
        }
        // A fenced, superseded or excluded claim must not send another chunk. Surfaced
        // as an error rather than a hold: fail/finish would be rejected on the same
        // stale token, and the job is left for a later claim after its lease.
        state.renew_extraction(&claim, ingestion::now(), 3600)?;
        // Recheck before every transmission, including after a prior chunk.
        if backend.external() && !state.external_allowed(project)? {
            state.fail_extraction(
                &claim,
                "held_external_policy",
                false,
                &provenance,
                ingestion::now(),
            )?;
            return Ok(true);
        }
        let supplied = Evidence {
            items: items[range].to_vec(),
            ..metadata.clone()
        };
        provenance["chunk_index"] = json!(index);
        provenance["chunk_items"] = json!(supplied.items.len());
        let (prompt, schema, schema_bytes) =
            match chunk_request(project, &claim.evidence, &supplied) {
                Ok(request) => request,
                Err(reason) => {
                    state.fail_extraction(&claim, reason, false, &provenance, ingestion::now())?;
                    return Ok(true);
                }
            };
        provenance["input_bytes"] = json!(prompt.len() + schema_bytes.len());
        provenance["input_hash"] = json!(hash(prompt.as_bytes()));
        provenance["schema_hash"] = json!(hash(&schema_bytes));
        // Output fields describe this chunk only, even if a later call fails.
        for key in ["output_hash", "generation_thread", "runtime_version"] {
            provenance.as_object_mut().unwrap().remove(key);
        }
        let request = Request {
            attempt: format!("extraction-{}-{}-{}", claim.key.id(), claim.token, index),
            prompt,
            schema,
        };
        let output = match backend.generate(request).await {
            Ok(output) => output,
            Err(failure) => {
                state.fail_extraction(
                    &claim,
                    failure.reason(),
                    failure.retryable(),
                    &provenance,
                    ingestion::now(),
                )?;
                return Ok(true);
            }
        };
        provenance["output_hash"] = json!(hash(&output.bytes));
        provenance["generation_thread"] = json!(output.thread_id);
        provenance["runtime_version"] = json!(output.runtime_version);
        let chunk_candidates = match extraction::decode(&output.bytes, &supplied, &claim.evidence) {
            Ok(candidates) => candidates,
            Err(_) => {
                state.fail_extraction(
                    &claim,
                    "held_invalid_candidates",
                    false,
                    &provenance,
                    ingestion::now(),
                )?;
                return Ok(true);
            }
        };
        if extraction::merge(&mut candidates, chunk_candidates).is_err() {
            state.fail_extraction(
                &claim,
                "held_invalid_candidates",
                false,
                &provenance,
                ingestion::now(),
            )?;
            return Ok(true);
        }
        provenance["completed_chunks"] = json!(index + 1);
        // The prefix is durable before the next call, so a later failure cannot repeat calls
        // that already succeeded. It publishes no candidate until the final commit.
        state.record_extraction_progress(
            &claim,
            &plan,
            index + 1,
            &candidates,
            ingestion::now(),
        )?;
    }
    // Revocation during generation prevents staging; it cannot undo a request already sent.
    if backend.external() && !state.external_allowed(project)? {
        state.fail_extraction(
            &claim,
            "held_external_policy",
            false,
            &provenance,
            ingestion::now(),
        )?;
    } else {
        state.finish_extraction(&claim, &candidates, &provenance, ingestion::now())?;
    }
    Ok(true)
}

/// Partition signature recorded with a resumable prefix. Boundaries are hashed in order so
/// an Engine change that repartitions the same evidence cannot reuse an incompatible prefix.
fn plan_signature(ranges: &[std::ops::Range<usize>]) -> String {
    let mut bytes = Vec::with_capacity(ranges.len() * 16);
    for range in ranges {
        bytes.extend_from_slice(&(range.start as u64).to_le_bytes());
        bytes.extend_from_slice(&(range.end as u64).to_le_bytes());
    }
    hash(&bytes)
}

/// Build one chunk request. `chunks` already proves the supplied items fit the input
/// budget, so a failure here is defensive: it is still reported as a bounded evidence
/// hold instead of leaving the claimed job `running`.
fn chunk_request(
    project: &str,
    digest: &str,
    supplied: &Evidence,
) -> std::result::Result<(String, Value, Vec<u8>), &'static str> {
    let prompt = extraction::prompt(supplied, digest).map_err(|_| "held_invalid_evidence")?;
    let schema = extraction::schema(project, digest, &supplied.items);
    let schema_bytes = serde_json::to_vec(&schema).map_err(|_| "held_invalid_evidence")?;
    Ok((prompt, schema, schema_bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn unframeable_chunk_is_a_bounded_evidence_hold() -> Result<()> {
        let supplied = Evidence {
            schema_version: 1,
            run: crate::domain::RunKey {
                store: "source".into(),
                thread: "thread".into(),
                turn: "turn".into(),
            },
            project: "alpha".into(),
            runtime_version: "fixture".into(),
            capture_runtime: None,
            outcome: "completed".into(),
            started_at: 1,
            completed_at: 2,
            cwd: None,
            terminal_error: None,
            items: vec![
                json!({"id":"E1","type":"commandExecution","text":"x".repeat(extraction::MAX_INPUT)}),
            ],
            coverage: vec![],
        };
        // `chunks` rejects this item; the request builder must never leave the job without
        // a reason if it is reached anyway.
        assert_eq!(
            chunk_request("alpha", &"a".repeat(64), &supplied).unwrap_err(),
            "held_invalid_evidence"
        );
        Ok(())
    }
}
