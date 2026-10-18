//! One bounded extraction attempt; SQLite transactions never span generation.
use crate::{
    domain::{Evidence, hash},
    evidence, extraction,
    generation::{Backend, Request},
    ingestion,
    state::State,
};
use anyhow::{Result, ensure};
use serde_json::json;
use std::path::Path;

pub async fn extract_one(
    state: &mut State,
    root: &Path,
    project: &str,
    backend: &impl Backend,
) -> Result<bool> {
    let Some(claim) = state.claim_extraction(project, ingestion::now(), 3600)? else {
        return Ok(false);
    };
    let mut provenance = backend.provenance();
    ensure!(
        provenance.is_object(),
        "Backend provenance must be an object"
    );
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
    let input = (|| -> Result<_> {
        ensure!(
            claim.evidence.len() == 64
                && claim
                    .evidence
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "Invalid evidence identity"
        );
        let path = root
            .join("evidence")
            .join(format!("{}.json", claim.evidence));
        let metadata = std::fs::metadata(&path)?;
        provenance["source_bytes"] = json!(metadata.len());
        provenance["source_limit"] = json!(extraction::MAX_SOURCE_BYTES);
        ensure!(
            metadata.len() <= extraction::MAX_SOURCE_BYTES as u64,
            "Source exceeds local limit"
        );
        let bytes = evidence::read_bounded(&path, extraction::MAX_SOURCE_BYTES)?;
        provenance["source_bytes"] = json!(bytes.len());
        ensure!(hash(&bytes) == claim.evidence, "Corrupt captured evidence");
        let bundle: Evidence = serde_json::from_slice(&bytes)?;
        ensure!(
            bundle.schema_version == 1
                && bundle.run == claim.key
                && bundle.project == claim.project
                && bundle.outcome == "completed",
            "Evidence does not match captured run"
        );
        let mut ids = std::collections::HashSet::new();
        for item in &bundle.items {
            let id = item["id"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("Evidence item lacks identity"))?;
            ensure!(
                !id.is_empty() && ids.insert(id),
                "Ambiguous evidence identity"
            );
        }
        Ok(bundle)
    })();
    let bundle = match input {
        Ok(input) => input,
        Err(error) => {
            let reason = if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound)
            {
                "held_missing_evidence"
            } else if provenance["source_bytes"]
                .as_u64()
                .is_some_and(|n| n > extraction::MAX_SOURCE_BYTES as u64)
            {
                "held_oversized_evidence_source"
            } else {
                "held_invalid_evidence"
            };
            state.fail_extraction(&claim, reason, false, &provenance, ingestion::now())?;
            return Ok(true);
        }
    };
    provenance["input_limit"] = json!(extraction::MAX_INPUT);
    provenance["item_count"] = json!(bundle.items.len());
    let ranges = match extraction::chunks(&bundle, &claim.evidence) {
        Ok(ranges) => ranges,
        Err(reason) => {
            state.fail_extraction(&claim, reason, false, &provenance, ingestion::now())?;
            return Ok(true);
        }
    };
    provenance["chunk_count"] = json!(ranges.len());
    provenance["completed_chunks"] = json!(0);
    let mut metadata = bundle;
    let items = std::mem::take(&mut metadata.items);
    let mut candidates = Vec::new();
    for (index, range) in ranges.into_iter().enumerate() {
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
        let prompt = extraction::prompt(&supplied, &claim.evidence)?;
        let schema = extraction::schema(project, &claim.evidence, &supplied.items);
        let schema_bytes = serde_json::to_vec(&schema)?;
        provenance["chunk_index"] = json!(index);
        provenance["chunk_items"] = json!(supplied.items.len());
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
        extraction::merge(&mut candidates, chunk_candidates)?;
        provenance["completed_chunks"] = json!(index + 1);
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
