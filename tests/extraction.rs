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
        Some("held_invalid_or_oversized_evidence")
    );
    Ok(())
}
