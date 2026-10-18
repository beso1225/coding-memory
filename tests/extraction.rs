use anyhow::Result;
use coding_memory::{
    domain::{Evidence, RunKey},
    evidence,
    extraction::{self, Batch, CandidateKind},
    state::State,
};
use serde_json::json;
use tempfile::tempdir;

fn bundle() -> Evidence {
    Evidence {
        schema_version: 1,
        run: RunKey {
            store: "source".into(),
            thread: "thread".into(),
            turn: "turn".into(),
        },
        project: "alpha".into(),
        runtime_version: "fixture".into(),
        capture_runtime: None,
        outcome: "completed".into(),
        started_at: 10,
        completed_at: 11,
        cwd: None,
        terminal_error: None,
        items: vec![
            json!({"id":"E1","type":"commandExecution","status":"completed","exitCode":0,"aggregatedOutput":"PASS"}),
            json!({"id":"U1","type":"userMessage","content":[{"type":"text","text":"Use a token-fenced lease"}]}),
            json!({"id":"A1","type":"agentMessage","text":"Unverified speculation"}),
        ],
        coverage: vec![],
    }
}
fn output(digest: &str) -> serde_json::Value {
    json!({"schema_version":1,"role":"extraction","project_id":"alpha","candidates":[{"kind":"procedure","title":"Recover an expired claim","claim":"Use a token-fenced capture lease.","scope":{"kind":"project","project_id":"alpha"},"conditions":["SQLite WAL and a persisted lease token"],"verification":"observed","evidence":[{"bundle":digest,"item":"E1"}]}]})
}

#[test]
fn candidates_preserve_conditions_and_reject_invalid_scope_and_locators() -> Result<()> {
    let d = tempdir()?;
    let b = bundle();
    let digest = evidence::save(d.path(), &b)?;
    let value = output(&digest);
    let batch: Batch = serde_json::from_value(value.clone())?;
    let validated = extraction::validate(batch, &b, &digest)?;
    assert_eq!(
        validated[0].candidate.conditions,
        vec!["SQLite WAL and a persisted lease token"]
    );
    assert_eq!(validated[0].candidate.kind, CandidateKind::Procedure);
    assert_eq!(validated[0].state, "ready");
    let mut invalid = value.clone();
    invalid["candidates"][0]["evidence"][0]["item"] = json!("missing");
    assert!(extraction::validate(serde_json::from_value(invalid)?, &b, &digest).is_err());
    let mut wrong = value.clone();
    wrong["candidates"][0]["scope"]["project_id"] = json!("beta");
    assert!(extraction::validate(serde_json::from_value(wrong)?, &b, &digest).is_err());
    let mut progress = value.clone();
    progress["candidates"][0]["kind"] = json!("progress");
    assert!(serde_json::from_value::<Batch>(progress).is_err());
    let empty: Batch = serde_json::from_value(
        json!({"schema_version":1,"role":"extraction","project_id":"alpha","candidates":[]}),
    )?;
    assert!(extraction::validate(empty, &b, &digest)?.is_empty());
    Ok(())
}

#[test]
fn unverified_or_nonobservational_support_is_held() -> Result<()> {
    let d = tempdir()?;
    let b = bundle();
    let digest = evidence::save(d.path(), &b)?;
    let mut value = output(&digest);
    value["candidates"][0]["verification"] = json!("unverified");
    assert_eq!(
        extraction::validate(serde_json::from_value(value)?, &b, &digest)?[0].state,
        "held"
    );
    let mut inferred = output(&digest);
    inferred["candidates"][0]["evidence"][0]["item"] = json!("A1");
    assert_eq!(
        extraction::validate(serde_json::from_value(inferred)?, &b, &digest)?[0].state,
        "held"
    );
    let mut stated = output(&digest);
    stated["candidates"][0]["verification"] = json!("user_stated");
    stated["candidates"][0]["evidence"][0]["item"] = json!("U1");
    assert_eq!(
        extraction::validate(serde_json::from_value(stated)?, &b, &digest)?[0].state,
        "ready"
    );
    Ok(())
}

#[test]
fn extraction_claims_are_fenced_and_candidates_commit_atomically() -> Result<()> {
    let d = tempdir()?;
    let b = bundle();
    let digest = evidence::save(d.path(), &b)?;
    let mut state = State::open(&d.path().join("state.sqlite"))?;
    state.register_project("alpha", "/repo", 1)?;
    state.enqueue(&b.run, "alpha", "/repo", 2)?;
    let capture = state.claim(3, 10)?.unwrap();
    state.finish(&capture, "completed", Some(&digest), 4)?;
    assert_eq!(state.enqueue_extractions("alpha", 5)?, 1);
    assert_eq!(state.enqueue_extractions("alpha", 5)?, 0);
    let first = state.claim_extraction("alpha", 6, 10)?.unwrap();
    let next = state.claim_extraction("alpha", 16, 10)?.unwrap();
    let validated = extraction::validate(serde_json::from_value(output(&digest))?, &b, &digest)?;
    let provenance =
        json!({"backend":"fixture","model":"synthetic","prompt_version":"v1","schema_version":1});
    assert!(
        state
            .finish_extraction(&first, &validated, &provenance, 17)
            .is_err()
    );
    state.finish_extraction(&next, &validated, &provenance, 17)?;
    assert_eq!(state.candidates("alpha")?.len(), 1);
    assert_eq!(state.extraction_status("alpha")?[0].state, "succeeded");
    assert!(state.claim_extraction("alpha", 30, 10)?.is_none());
    Ok(())
}

#[test]
fn explicit_project_permission_is_persistent_and_defaults_to_denied() -> Result<()> {
    let d = tempdir()?;
    let path = d.path().join("state.sqlite");
    let mut state = State::open(&path)?;
    state.register_project("alpha", "/repo", 1)?;
    assert!(!state.external_allowed("alpha")?);
    state.set_external_policy("alpha", true)?;
    drop(state);
    assert!(State::open(&path)?.external_allowed("alpha")?);
    Ok(())
}

struct FixtureBackend {
    output: std::result::Result<Vec<u8>, coding_memory::generation::Failure>,
    calls: std::sync::atomic::AtomicUsize,
}
impl coding_memory::generation::Backend for FixtureBackend {
    fn external(&self) -> bool {
        true
    }
    fn provenance(&self) -> serde_json::Value {
        json!({"backend":"fixture","model":"synthetic","prompt_version":extraction::PROMPT_VERSION,"schema_version":1})
    }
    async fn generate(
        &self,
        request: coding_memory::generation::Request,
    ) -> std::result::Result<coding_memory::generation::Output, coding_memory::generation::Failure>
    {
        self.calls
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        assert!(request.prompt.contains("untrusted recorded evidence"));
        assert_eq!(
            request.schema["properties"]["role"]["enum"][0],
            "extraction"
        );
        self.output
            .clone()
            .map(|bytes| coding_memory::generation::Output {
                bytes,
                thread_id: Some("generation-thread".into()),
                runtime_version: "fixture".into(),
            })
    }
}
fn captured(root: &std::path::Path) -> Result<(State, String)> {
    let b = bundle();
    let digest = evidence::save(root, &b)?;
    let mut state = State::open(&root.join("state.sqlite"))?;
    state.register_project("alpha", "/repo", 1)?;
    state.enqueue(&b.run, "alpha", "/repo", 2)?;
    let c = state.claim(3, 10)?.unwrap();
    state.finish(&c, "completed", Some(&digest), 4)?;
    state.enqueue_extractions("alpha", 5)?;
    Ok((state, digest))
}
#[tokio::test]
async fn worker_gates_transmission_and_stages_with_provenance() -> Result<()> {
    let d = tempdir()?;
    let (mut state, digest) = captured(d.path())?;
    let backend = FixtureBackend {
        output: Ok(serde_json::to_vec(&output(&digest))?),
        calls: 0.into(),
    };
    assert!(
        coding_memory::extraction_worker::extract_one(&mut state, d.path(), "alpha", &backend)
            .await?
    );
    assert_eq!(backend.calls.load(std::sync::atomic::Ordering::Relaxed), 0);
    assert_eq!(
        state.extraction_status("alpha")?[0].reason.as_deref(),
        Some("held_external_policy")
    );
    state.set_external_policy("alpha", true)?;
    assert!(
        coding_memory::extraction_worker::extract_one(&mut state, d.path(), "alpha", &backend)
            .await?
    );
    assert_eq!(state.candidates("alpha")?.len(), 1);
    let status = state.extraction_status("alpha")?;
    let p: serde_json::Value = serde_json::from_str(status[0].provenance.as_ref().unwrap())?;
    assert_eq!(p["model"], "synthetic");
    assert_eq!(p["generation_thread"], "generation-thread");
    for field in ["input_hash", "schema_hash", "output_hash", "evidence_hash"] {
        assert_eq!(p[field].as_str().unwrap().len(), 64);
    }
    assert!(
        !coding_memory::extraction_worker::extract_one(&mut state, d.path(), "alpha", &backend)
            .await?
    );
    Ok(())
}
#[tokio::test]
async fn empty_invalid_and_failed_extractions_remain_distinct() -> Result<()> {
    use coding_memory::generation::Failure;
    for (out, expected) in [
        (
            Ok(serde_json::to_vec(
                &json!({"schema_version":1,"role":"extraction","project_id":"alpha","candidates":[]}),
            )?),
            "succeeded_empty",
        ),
        (Ok(b"not JSON".to_vec()), "held"),
        (Err(Failure::ProcessFailure), "queued"),
        (Err(Failure::ToolAttempt), "held"),
    ] {
        let d = tempdir()?;
        let (mut state, _) = captured(d.path())?;
        state.set_external_policy("alpha", true)?;
        let backend = FixtureBackend {
            output: out,
            calls: 0.into(),
        };
        coding_memory::extraction_worker::extract_one(&mut state, d.path(), "alpha", &backend)
            .await?;
        assert_eq!(state.extraction_status("alpha")?[0].state, expected);
        assert!(state.candidates("alpha")?.is_empty());
    }
    Ok(())
}
#[test]
fn retryable_failures_are_bounded_and_atomic_insert_rolls_back() -> Result<()> {
    let d = tempdir()?;
    let (mut state, digest) = captured(d.path())?;
    let candidate =
        extraction::validate(serde_json::from_value(output(&digest))?, &bundle(), &digest)?;
    let c = state.claim_extraction("alpha", 10, 10)?.unwrap();
    let duplicate = vec![candidate[0].clone(), candidate[0].clone()];
    assert!(
        state
            .finish_extraction(&c, &duplicate, &json!({}), 11)
            .is_err()
    );
    assert!(state.candidates("alpha")?.is_empty());
    assert_eq!(state.extraction_status("alpha")?[0].state, "running");
    state.fail_extraction(&c, "generation_timeout", true, &json!({}), 11)?;
    assert!(state.claim_extraction("alpha", 20, 10)?.is_none());
    let c = state.claim_extraction("alpha", 21, 10)?.unwrap();
    state.fail_extraction(&c, "generation_timeout", true, &json!({}), 22)?;
    let c = state.claim_extraction("alpha", 32, 10)?.unwrap();
    state.fail_extraction(&c, "generation_timeout", true, &json!({}), 33)?;
    assert_eq!(state.extraction_status("alpha")?[0].state, "exhausted");
    state.retry_extraction(&bundle().run, 40)?;
    assert_eq!(
        state.claim_extraction("alpha", 40, 10)?.unwrap().attempts,
        1
    );
    Ok(())
}

#[test]
fn repeated_worker_crashes_exhaust_the_job_and_owned_sources_never_generate() -> Result<()> {
    let d = tempdir()?;
    let (mut state, _) = captured(d.path())?;
    for now in [10, 20, 30] {
        assert!(state.claim_extraction("alpha", now, 10)?.is_some());
    }
    assert!(state.claim_extraction("alpha", 40, 10)?.is_none());
    assert_eq!(state.extraction_status("alpha")?[0].state, "exhausted");
    state.retry_extraction(&bundle().run, 50)?;
    state.own_store("source")?;
    assert!(state.claim_extraction("alpha", 50, 10)?.is_none());
    assert_eq!(
        state.extraction_status("alpha")?[0].reason.as_deref(),
        Some("source_excluded")
    );
    Ok(())
}

#[test]
fn revoked_permission_prevents_candidate_insertion_at_commit() -> Result<()> {
    let d = tempdir()?;
    let (mut state, digest) = captured(d.path())?;
    state.set_external_policy("alpha", true)?;
    let c = state.claim_extraction("alpha", 10, 10)?.unwrap();
    let validated =
        extraction::validate(serde_json::from_value(output(&digest))?, &bundle(), &digest)?;
    state.set_external_policy("alpha", false)?;
    state.finish_extraction(&c, &validated, &json!({"external_transmission":true}), 11)?;
    assert!(state.candidates("alpha")?.is_empty());
    assert_eq!(
        state.extraction_status("alpha")?[0].reason.as_deref(),
        Some("held_external_policy")
    );
    Ok(())
}

#[test]
fn concurrent_allow_cannot_leave_a_job_policy_held() -> Result<()> {
    let d = tempdir()?;
    let (mut state, _) = captured(d.path())?;
    let c = state.claim_extraction("alpha", 10, 10)?.unwrap();
    assert!(!state.external_allowed("alpha")?);
    // Another CLI changes policy between the worker's read and hold transition.
    let mut admin = State::open(&d.path().join("state.sqlite"))?;
    admin.set_external_policy("alpha", true)?;
    state.fail_extraction(&c, "held_external_policy", false, &json!({}), 11)?;
    assert_eq!(state.extraction_status("alpha")?[0].state, "queued");
    assert_eq!(
        state.claim_extraction("alpha", 11, 10)?.unwrap().attempts,
        1
    );
    Ok(())
}

#[tokio::test]
async fn missing_evidence_is_held_before_backend_invocation() -> Result<()> {
    let d = tempdir()?;
    let (mut state, digest) = captured(d.path())?;
    state.set_external_policy("alpha", true)?;
    std::fs::remove_file(d.path().join("evidence").join(format!("{digest}.json")))?;
    let backend = FixtureBackend {
        output: Ok(vec![]),
        calls: 0.into(),
    };
    coding_memory::extraction_worker::extract_one(&mut state, d.path(), "alpha", &backend).await?;
    assert_eq!(backend.calls.load(std::sync::atomic::Ordering::Relaxed), 0);
    assert_eq!(
        state.extraction_status("alpha")?[0].reason.as_deref(),
        Some("held_missing_evidence")
    );
    Ok(())
}

#[tokio::test]
async fn empty_bundle_completes_without_a_model_call() -> Result<()> {
    let d = tempdir()?;
    let mut b = bundle();
    b.items.clear();
    let (mut state, _) = captured_bundle(d.path(), &b)?;
    let backend = ChunkBackend {
        calls: 0.into(),
        fail_at: None,
        omitted_citation: false,
    };
    assert!(
        coding_memory::extraction_worker::extract_one(&mut state, d.path(), "alpha", &backend)
            .await?
    );
    assert_eq!(backend.calls.load(std::sync::atomic::Ordering::Relaxed), 0);
    let status = state.extraction_status("alpha")?;
    assert_eq!(status[0].state, "succeeded_empty");
    assert_eq!(status[0].reason.as_deref(), None);
    let p: serde_json::Value = serde_json::from_str(status[0].provenance.as_ref().unwrap())?;
    assert_eq!(p["item_count"], 0);
    assert_eq!(p["chunk_count"], 0);
    assert_eq!(p["completed_chunks"], 0);
    assert!(state.candidates("alpha")?.is_empty());
    Ok(())
}

#[tokio::test]
async fn unreadable_evidence_is_held_without_sharing_the_corruption_reason() -> Result<()> {
    let d = tempdir()?;
    let (mut state, digest) = captured(d.path())?;
    state.set_external_policy("alpha", true)?;
    let path = d.path().join("evidence").join(format!("{digest}.json"));
    // A directory can be measured as small but not read as a file, so this exercises the
    // read failure without depending on file permissions.
    std::fs::remove_file(&path)?;
    std::fs::create_dir(&path)?;
    let backend = ChunkBackend {
        calls: 0.into(),
        fail_at: None,
        omitted_citation: false,
    };
    coding_memory::extraction_worker::extract_one(&mut state, d.path(), "alpha", &backend).await?;
    assert_eq!(backend.calls.load(std::sync::atomic::Ordering::Relaxed), 0);
    let status = state.extraction_status("alpha")?;
    assert_eq!(
        status[0].reason.as_deref(),
        Some("held_unreadable_evidence")
    );
    let p: serde_json::Value = serde_json::from_str(status[0].provenance.as_ref().unwrap())?;
    assert!(p["source_bytes"].as_u64().is_some());
    assert_eq!(
        p["source_limit"].as_u64(),
        Some(evidence::MAX_SOURCE_BYTES as u64)
    );
    Ok(())
}

fn large_bundle() -> Evidence {
    let mut b = bundle();
    b.items = (0..3).map(|i| json!({"id":format!("E{i}"),"type":"commandExecution","status":"completed","text":"界\\\"".repeat(14000)})).collect();
    b
}

#[test]
fn chunks_budget_framing_schema_and_preserve_every_native_item() -> Result<()> {
    let b = large_bundle();
    let digest = "a".repeat(64);
    let ranges = extraction::chunks(&b, &digest).map_err(anyhow::Error::msg)?;
    assert!(ranges.len() > 1);
    let mut ids = Vec::new();
    for range in ranges {
        let mut supplied = b.clone();
        supplied.items = b.items[range].to_vec();
        let prompt = extraction::prompt(&supplied, &digest)?;
        let schema = extraction::schema(&b.project, &digest, &supplied.items);
        assert!(prompt.len() + serde_json::to_vec(&schema)?.len() <= extraction::MAX_INPUT);
        ids.extend(
            supplied
                .items
                .iter()
                .map(|v| v["id"].as_str().unwrap().to_owned()),
        );
        let mut omitted = output(&digest);
        omitted["candidates"][0]["evidence"][0]["item"] = json!("not-supplied");
        assert!(extraction::decode(&serde_json::to_vec(&omitted)?, &supplied, &digest).is_err());
    }
    assert_eq!(ids, vec!["E0", "E1", "E2"]);
    Ok(())
}

struct ChunkBackend {
    calls: std::sync::atomic::AtomicUsize,
    fail_at: Option<usize>,
    omitted_citation: bool,
}
impl coding_memory::generation::Backend for ChunkBackend {
    fn external(&self) -> bool {
        true
    }
    fn provenance(&self) -> serde_json::Value {
        json!({"backend":"fixture"})
    }
    async fn generate(
        &self,
        request: coding_memory::generation::Request,
    ) -> std::result::Result<coding_memory::generation::Output, coding_memory::generation::Failure>
    {
        use std::sync::atomic::Ordering;
        let n = self.calls.fetch_add(1, Ordering::Relaxed);
        assert!(
            request.prompt.len() + serde_json::to_vec(&request.schema).unwrap().len()
                <= extraction::MAX_INPUT
        );
        if self.fail_at == Some(n) {
            return Err(coding_memory::generation::Failure::ProcessFailure);
        }
        let input: serde_json::Value =
            serde_json::from_str(request.prompt.split_once("INPUT_JSON\n").unwrap().1).unwrap();
        let mut out = output(input["bundle"].as_str().unwrap());
        let id = if self.omitted_citation {
            "E2"
        } else {
            input["evidence"]["items"][0]["id"].as_str().unwrap()
        };
        out["candidates"][0]["evidence"][0]["item"] = json!(id);
        Ok(coding_memory::generation::Output {
            bytes: serde_json::to_vec(&out).unwrap(),
            thread_id: None,
            runtime_version: "fixture".into(),
        })
    }
}
fn captured_bundle(root: &std::path::Path, b: &Evidence) -> Result<(State, String)> {
    let digest = evidence::save(root, b)?;
    let mut state = State::open(&root.join("state.sqlite"))?;
    state.register_project("alpha", "/repo", 1)?;
    state.enqueue(&b.run, "alpha", "/repo", 2)?;
    let c = state.claim(3, 10)?.unwrap();
    state.finish(&c, "completed", Some(&digest), 4)?;
    state.enqueue_extractions("alpha", 5)?;
    state.set_external_policy("alpha", true)?;
    Ok((state, digest))
}
#[tokio::test]
async fn chunk_failure_stages_nothing_and_deliberate_retry_commits_once() -> Result<()> {
    let d = tempdir()?;
    let b = large_bundle();
    let (mut state, digest) = captured_bundle(d.path(), &b)?;
    let path = d.path().join("evidence").join(format!("{digest}.json"));
    let before = std::fs::read(&path)?;
    assert!(before.len() > extraction::MAX_INPUT);
    let backend = ChunkBackend {
        calls: 0.into(),
        fail_at: Some(1),
        omitted_citation: false,
    };
    coding_memory::extraction_worker::extract_one(&mut state, d.path(), "alpha", &backend).await?;
    assert_eq!(backend.calls.load(std::sync::atomic::Ordering::Relaxed), 2);
    assert!(state.candidates("alpha")?.is_empty());
    assert_eq!(state.extraction_status("alpha")?[0].state, "queued");
    // Exhaustion/holds use the existing explicit retry interface; no schema migration.
    let c = state
        .claim_extraction("alpha", coding_memory::ingestion::now() + 20, 3600)?
        .unwrap();
    state.fail_extraction(
        &c,
        "held_invalid_or_oversized_evidence",
        false,
        &json!({}),
        coding_memory::ingestion::now() + 21,
    )?;
    state.retry_extraction(&b.run, coding_memory::ingestion::now())?;
    let backend = ChunkBackend {
        calls: 0.into(),
        fail_at: None,
        omitted_citation: false,
    };
    coding_memory::extraction_worker::extract_one(&mut state, d.path(), "alpha", &backend).await?;
    assert_eq!(state.extraction_status("alpha")?[0].state, "succeeded");
    assert_eq!(
        state.candidates("alpha")?.len(),
        extraction::chunks(&b, &digest)
            .map_err(anyhow::Error::msg)?
            .len()
    );
    assert_eq!(std::fs::read(path)?, before);
    let status = state.extraction_status("alpha")?;
    let provenance: serde_json::Value =
        serde_json::from_str(status[0].provenance.as_ref().unwrap())?;
    assert_eq!(provenance["source_bytes"], before.len());
    assert_eq!(provenance["item_count"], b.items.len());
    assert_eq!(provenance["completed_chunks"], provenance["chunk_count"]);
    assert!(provenance["input_bytes"].as_u64().unwrap() <= extraction::MAX_INPUT as u64);
    assert!(provenance.to_string().len() < 2048);
    assert!(
        !coding_memory::extraction_worker::extract_one(&mut state, d.path(), "alpha", &backend)
            .await?
    );
    Ok(())
}
#[tokio::test]
async fn omitted_original_item_and_oversized_single_item_are_held() -> Result<()> {
    for oversized in [false, true] {
        let d = tempdir()?;
        let mut b = large_bundle();
        if oversized {
            b.items[2]["text"] = json!("x".repeat(extraction::MAX_INPUT));
        }
        let (mut state, _) = captured_bundle(d.path(), &b)?;
        let backend = ChunkBackend {
            calls: 0.into(),
            fail_at: None,
            omitted_citation: true,
        };
        coding_memory::extraction_worker::extract_one(&mut state, d.path(), "alpha", &backend)
            .await?;
        assert!(state.candidates("alpha")?.is_empty());
        assert_eq!(
            state.extraction_status("alpha")?[0].reason.as_deref(),
            Some(if oversized {
                "held_oversized_evidence_item"
            } else {
                "held_invalid_candidates"
            })
        );
        if oversized {
            assert_eq!(backend.calls.load(std::sync::atomic::Ordering::Relaxed), 0);
        }
    }
    Ok(())
}

#[test]
fn exact_budget_boundary_empty_and_framing_overflow() -> Result<()> {
    let mut b = bundle();
    b.items = vec![json!({"id":"E1", "text":""})];
    let digest = "a".repeat(64);
    let used = extraction::prompt(&b, &digest)?.len()
        + serde_json::to_vec(&extraction::schema(&b.project, &digest, &b.items))?.len();
    b.items[0]["text"] = json!("x".repeat(extraction::MAX_INPUT - used));
    assert_eq!(extraction::chunks(&b, &digest).unwrap(), vec![0..1]);
    assert!(extraction::prompt(&b, &digest).is_ok());
    b.items.push(json!({"id":"next", "text":"small"}));
    assert_eq!(extraction::chunks(&b, &digest).unwrap(), vec![0..1, 1..2]);
    assert!(extraction::prompt(&b, &digest).is_err());
    b.items.pop();
    b.items[0]["text"] = json!("x".repeat(extraction::MAX_INPUT - used + 1));
    assert_eq!(
        extraction::chunks(&b, &digest).unwrap_err(),
        "held_oversized_evidence_item"
    );
    b.items.clear();
    assert_eq!(extraction::chunks(&b, &digest).unwrap(), vec![0..0]);
    b.coverage = vec!["x".repeat(extraction::MAX_INPUT)];
    assert_eq!(
        extraction::chunks(&b, &digest).unwrap_err(),
        "held_oversized_evidence_framing"
    );
    Ok(())
}
#[test]
fn merge_deduplicates_exact_candidates_without_losing_distinct_support() -> Result<()> {
    let digest = "a".repeat(64);
    let one = extraction::decode(&serde_json::to_vec(&output(&digest))?, &bundle(), &digest)?;
    let mut merged = Vec::new();
    extraction::merge(&mut merged, one.clone())?;
    extraction::merge(&mut merged, one.clone())?;
    assert_eq!(merged.len(), 1);
    let mut other = one;
    other[0]
        .candidate
        .conditions
        .push("Additional applicability".into());
    extraction::merge(&mut merged, other)?;
    assert_eq!(merged.len(), 2);
    Ok(())
}
#[tokio::test]
async fn corrupt_unsupported_and_local_oversize_never_generate() -> Result<()> {
    for mode in ["corrupt", "unsupported", "oversize"] {
        let d = tempdir()?;
        let mut b = bundle();
        if mode == "unsupported" {
            b.schema_version = 99;
        }
        let (mut state, digest) = captured_bundle(d.path(), &b)?;
        let path = d.path().join("evidence").join(format!("{digest}.json"));
        if mode == "corrupt" {
            std::fs::write(&path, b"corrupt")?;
        }
        if mode == "oversize" {
            std::fs::OpenOptions::new()
                .write(true)
                .open(&path)?
                .set_len(evidence::MAX_SOURCE_BYTES as u64 + 1)?;
        }
        let backend = ChunkBackend {
            calls: 0.into(),
            fail_at: None,
            omitted_citation: false,
        };
        coding_memory::extraction_worker::extract_one(&mut state, d.path(), "alpha", &backend)
            .await?;
        assert_eq!(backend.calls.load(std::sync::atomic::Ordering::Relaxed), 0);
        assert_eq!(
            state.extraction_status("alpha")?[0].reason.as_deref(),
            Some(if mode == "oversize" {
                "held_oversized_evidence_source"
            } else {
                "held_invalid_evidence"
            })
        );
    }
    Ok(())
}
#[test]
fn chunk_lease_renewal_is_fenced_and_expiry_stays_bounded() -> Result<()> {
    let d = tempdir()?;
    let (mut state, _) = captured(d.path())?;
    let first = state.claim_extraction("alpha", 10, 10)?.unwrap();
    state.renew_extraction(&first, 19, 10)?;
    assert!(state.claim_extraction("alpha", 20, 10)?.is_none());
    let second = state.claim_extraction("alpha", 29, 10)?.unwrap();
    assert!(state.renew_extraction(&first, 30, 10).is_err());
    assert!(state.renew_extraction(&second, 39, 10).is_err());
    Ok(())
}

struct RevokingBackend {
    state_path: std::path::PathBuf,
    inner: ChunkBackend,
}
impl coding_memory::generation::Backend for RevokingBackend {
    fn external(&self) -> bool {
        true
    }
    fn provenance(&self) -> serde_json::Value {
        json!({"backend":"fixture"})
    }
    async fn generate(
        &self,
        request: coding_memory::generation::Request,
    ) -> std::result::Result<coding_memory::generation::Output, coding_memory::generation::Failure>
    {
        let out = coding_memory::generation::Backend::generate(&self.inner, request).await;
        State::open(&self.state_path)
            .unwrap()
            .set_external_policy("alpha", false)
            .unwrap();
        out
    }
}
#[tokio::test]
async fn revocation_between_chunks_stops_transmission_and_staging() -> Result<()> {
    let d = tempdir()?;
    let (mut state, _) = captured_bundle(d.path(), &large_bundle())?;
    let backend = RevokingBackend {
        state_path: d.path().join("state.sqlite"),
        inner: ChunkBackend {
            calls: 0.into(),
            fail_at: None,
            omitted_citation: false,
        },
    };
    coding_memory::extraction_worker::extract_one(&mut state, d.path(), "alpha", &backend).await?;
    assert_eq!(
        backend
            .inner
            .calls
            .load(std::sync::atomic::Ordering::Relaxed),
        1
    );
    assert!(state.candidates("alpha")?.is_empty());
    assert_eq!(
        state.extraction_status("alpha")?[0].reason.as_deref(),
        Some("held_external_policy")
    );
    Ok(())
}
