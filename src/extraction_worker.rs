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
        let bytes = evidence::read_bounded(
            &root
                .join("evidence")
                .join(format!("{}.json", claim.evidence)),
            extraction::MAX_INPUT,
        )?;
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
        let prompt = extraction::prompt(&bundle, &claim.evidence)?;
        let schema = extraction::schema(project, &claim.evidence, &bundle.items);
        Ok((bundle, prompt, schema))
    })();
    let (bundle, prompt, schema) = match input {
        Ok(input) => input,
        Err(_) => {
            state.fail_extraction(
                &claim,
                "held_invalid_or_oversized_evidence",
                false,
                &provenance,
                ingestion::now(),
            )?;
            return Ok(true);
        }
    };
    provenance["input_hash"] = json!(hash(prompt.as_bytes()));
    provenance["schema_hash"] = json!(hash(&serde_json::to_vec(&schema)?));
    let request = Request {
        attempt: format!("extraction-{}-{}", claim.key.id(), claim.token),
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
    let candidates = match extraction::decode(&output.bytes, &bundle, &claim.evidence) {
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
