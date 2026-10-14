use anyhow::Result;
use coding_memory::{
    consolidation_state::Change,
    domain::{Evidence, RunKey, hash},
    evidence, extraction, notes,
    search::{Scope, Search},
    state::State,
    vault::{self, Point},
};
use serde_json::json;
use std::fs;
use tempfile::tempdir;
fn candidate(state: &mut State, root: &std::path::Path, n: usize, now: i64) -> Result<()> {
    candidate_with_text(state, root, n, now, "Use leased jobs", "Use leased jobs.")
}
fn candidate_with_text(
    state: &mut State,
    root: &std::path::Path,
    n: usize,
    now: i64,
    title: &str,
    claim: &str,
) -> Result<()> {
    let b = Evidence {
        schema_version: 1,
        run: RunKey {
            store: "fixture".into(),
            thread: format!("t{n}"),
            turn: "r".into(),
        },
        project: "alpha".into(),
        runtime_version: "fixture".into(),
        capture_runtime: None,
        outcome: "completed".into(),
        started_at: 1,
        completed_at: 2,
        cwd: None,
        terminal_error: None,
        coverage: vec![],
        items: vec![json!({"id":"U","type":"userMessage","text":claim})],
    };
    let digest = evidence::save(root, &b)?;
    state.enqueue(&b.run, "alpha", "/repo", 2)?;
    let c = state.claim(3, 10)?.unwrap();
    state.finish(&c, "completed", Some(&digest), 4)?;
    state.enqueue_extractions("alpha", 5)?;
    let c = state.claim_extraction("alpha", 6, 100)?.unwrap();
    let v = extraction::decode(
        &serde_json::to_vec(
            &json!({"schema_version":1,"role":"extraction","project_id":"alpha","candidates":[{"kind":"constraint","title":title,"claim":claim,"scope":{"kind":"project","project_id":"alpha"},"conditions":["SQLite"],"verification":"user_stated","evidence":[{"bundle":digest,"item":"U"}]}]}),
        )?,
        &b,
        &digest,
    )?;
    state.finish_extraction(&c, &v, &json!({}), now)?;
    Ok(())
}
#[test]
fn trigger_is_per_project_and_held_batches_do_not_loop() -> Result<()> {
    let d = tempdir()?;
    let mut s = State::open(&d.path().join("state.sqlite"))?;
    s.register_project("alpha", "/repo", 1)?;
    candidate(&mut s, d.path(), 0, 10)?;
    assert!(s.reserve_batch("alpha", 609, false)?.is_none());
    let batch = s.reserve_batch("alpha", 610, false)?.unwrap();
    assert_eq!(batch.candidates.len(), 1);
    s.hold_batch(batch.id, "fixture", &json!({}))?;
    assert!(s.reserve_batch("alpha", 900, true)?.is_none());
    s.retry_batch(batch.id, 900)?;
    assert!(s.reserve_batch("alpha", 900, true)?.is_some());
    Ok(())
}
#[test]
fn recovery_recognizes_replacement_and_preserves_publication_barrier() -> Result<()> {
    for crash in [
        Point::BeforeReplace,
        Point::AfterReplace,
        Point::BeforeFinalize,
    ] {
        let d = tempdir()?;
        let root = d.path().join("state");
        let v = d.path().join("vault");
        fs::create_dir(&root)?;
        fs::create_dir(&v)?;
        let mut s = State::open(&root.join("state.sqlite"))?;
        s.register_project("alpha", "/repo", 1)?;
        candidate(&mut s, &root, 0, 10)?;
        let batch = s.reserve_batch("alpha", 20, true)?.unwrap();
        let note = notes::parse(include_str!("fixtures/vault/queue.md"))?;
        let content = notes::render(&note)?;
        s.save_plan(
            &batch,
            &v.canonicalize()?,
            &[Change {
                note_id: note.metadata.id.clone(),
                path: "queue-leases.md".into(),
                before: None,
                after: hash(content.as_bytes()),
                content,
            }],
            &[batch.candidates[0].id.clone()],
            &json!({}),
            21,
        )?;
        assert!(
            vault::apply(&mut s, &root, &v, batch.id, |p| {
                if p == crash {
                    anyhow::bail!("synthetic stop");
                }
                Ok(())
            })
            .is_err()
        );
        let search = Search::new(&root, &v)?;
        search.rebuild()?;
        assert!(search.read("queue-leases", &Scope::new("alpha")).is_err());
        drop(s);
        let mut s = State::open(&root.join("state.sqlite"))?;
        assert!(vault::apply(&mut s, &root, &v, batch.id, |_| Ok(()))?);
        assert!(search.read("queue-leases", &Scope::new("alpha")).is_ok());
        assert_eq!(s.candidates("alpha")?[0].state, "consumed");
        assert_eq!(
            fs::read_dir(&v)?
                .filter_map(|x| x.ok())
                .filter(|x| x.path().extension().is_some_and(|x| x == "md"))
                .count(),
            1
        );
    }
    Ok(())
}
#[test]
fn unexpected_content_holds_application_without_consuming_candidates() -> Result<()> {
    let d = tempdir()?;
    let root = d.path().join("state");
    let v = d.path().join("vault");
    fs::create_dir(&root)?;
    fs::create_dir(&v)?;
    let mut s = State::open(&root.join("state.sqlite"))?;
    s.register_project("alpha", "/repo", 1)?;
    candidate(&mut s, &root, 0, 10)?;
    let batch = s.reserve_batch("alpha", 20, true)?.unwrap();
    let content = include_str!("fixtures/vault/queue.md").to_owned();
    s.save_plan(
        &batch,
        &v.canonicalize()?,
        &[Change {
            note_id: "queue-leases".into(),
            path: "queue-leases.md".into(),
            before: None,
            after: hash(content.as_bytes()),
            content,
        }],
        &[batch.candidates[0].id.clone()],
        &json!({}),
        21,
    )?;
    fs::write(v.join("queue-leases.md"), "unexpected human edit")?;
    assert!(!vault::apply(&mut s, &root, &v, batch.id, |_| Ok(()))?);
    assert_eq!(s.consolidation_status()?[0].state, "conflict");
    assert_eq!(s.candidates("alpha")?[0].state, "reserved");
    Ok(())
}

type KnowledgeFixture = (
    tempfile::TempDir,
    State,
    coding_memory::consolidation_state::Batch,
    std::collections::HashMap<String, Evidence>,
    coding_memory::consolidation::Existing,
);
fn knowledge_fixture() -> Result<KnowledgeFixture> {
    let d = tempdir()?;
    let mut s = State::open(&d.path().join("state.sqlite"))?;
    s.register_project("alpha", "/repo", 1)?;
    candidate(&mut s, d.path(), 0, 10)?;
    let batch = s.reserve_batch("alpha", 20, true)?.unwrap();
    let c = &batch.candidates[0].candidate;
    let digest = c.evidence[0].bundle.clone();
    let bundle = evidence::load(d.path(), &digest)?;
    let mut old = notes::parse(include_str!("fixtures/vault/queue.md"))?;
    old.metadata.title = "Use leased jobs".into();
    old.metadata.summary = "Use leased jobs.".into();
    old.metadata.conditions = c.conditions.clone();
    old.metadata.evidence = c.evidence.clone();
    old.metadata.verification = c.verification.clone();
    old.metadata.created_at = 1;
    old.metadata.updated_at = 1;
    old.body = "Use leased jobs.".into();
    let markdown = notes::render(&old)?;
    let existing = coding_memory::consolidation::Existing {
        id: old.metadata.id,
        path: "queue.md".into(),
        revision: hash(markdown.as_bytes()),
        markdown,
    };
    Ok((d, s, batch, [(digest, bundle)].into(), existing))
}
fn proposal(
    batch: &coding_memory::consolidation_state::Batch,
    action: &str,
    target: Option<&str>,
) -> serde_json::Value {
    let c = &batch.candidates[0].candidate;
    json!({"schema_version":1,"role":"consolidation","operations":[{"id":"op1","action":action,"candidates":[batch.candidates[0].id],"target":target,"draft":{"title":c.title,"summary":c.claim,"scope":c.scope,"conditions":c.conditions,"verification":c.verification,"evidence":c.evidence,"related":[],"body":c.claim},"dependencies":[],"rationale":"Supported synthetic constraint"}]})
}
#[test]
fn all_operation_kinds_preserve_identity_supersession_and_review_boundaries() -> Result<()> {
    use coding_memory::consolidation::{self, Decisions, Proposal};
    let (_d, _s, batch, bundles, existing) = knowledge_fixture()?;
    let decisions: Decisions = serde_json::from_value(
        json!({"schema_version":1,"role":"review","decisions":[{"operation":"op1","decision":"accept","rationale":"Supported and conditions retained"}]}),
    )?;
    let reviewer = notes::Review {
        decision: "accepted".into(),
        backend: "fixture".into(),
        model: "review-model".into(),
        prompt_version: "review-v1".into(),
        reviewed_at: 100,
    };
    for action in ["create", "update", "supersede", "skip", "hold"] {
        let mut p = proposal(
            &batch,
            action,
            if action == "create" {
                None
            } else {
                Some("queue-leases")
            },
        );
        if ["skip", "hold"].contains(&action) {
            p["operations"][0]["draft"] = json!(null);
        }
        let p: Proposal = serde_json::from_value(p)?;
        consolidation::validate(&p, &batch, std::slice::from_ref(&existing), &bundles)?;
        let (changes, consumed) = consolidation::plan(
            &p,
            &decisions,
            &batch,
            std::slice::from_ref(&existing),
            &reviewer,
            100,
        )?;
        if action == "hold" {
            assert!(changes.is_empty() && consumed.is_empty());
            continue;
        }
        assert_eq!(consumed.len(), 1);
        if action == "update" {
            assert_eq!(changes[0].note_id, "queue-leases");
            assert_eq!(
                changes[0].before.as_deref(),
                Some(existing.revision.as_str())
            );
        }
        if action == "supersede" {
            assert_eq!(changes.len(), 2);
            let old = notes::parse(&changes[1].content)?;
            assert_eq!(
                old.metadata.successor.as_deref(),
                Some(changes[0].note_id.as_str())
            );
            assert_eq!(old.metadata.status, notes::Status::Superseded);
        }
        if action == "create" {
            let note = notes::parse(&changes[0].content)?;
            assert_eq!(note.metadata.verification, notes::Verification::UserStated);
            assert_eq!(note.metadata.review.model, "review-model");
        }
    }
    Ok(())
}
#[test]
fn invalid_support_conditions_links_and_rejected_dependencies_cannot_publish() -> Result<()> {
    use coding_memory::consolidation::{self, Decisions, Proposal};
    let (_d, _s, mut batch, bundles, existing) = knowledge_fixture()?;
    let good = proposal(&batch, "create", None);
    for field in ["conditions", "evidence", "related", "scope"] {
        let mut bad = good.clone();
        bad["operations"][0]["draft"][field] = match field {
            "scope" => json!({"kind":"global"}),
            "related" => json!(["missing"]),
            _ => json!([]),
        };
        assert!(
            consolidation::validate(
                &serde_json::from_value(bad)?,
                &batch,
                std::slice::from_ref(&existing),
                &bundles
            )
            .is_err()
        );
    }
    let mut second = batch.candidates[0].candidate.clone();
    second.title = "Second independent knowledge".into();
    batch
        .candidates
        .push(coding_memory::extraction_state::StagedCandidate {
            id: "second".into(),
            state: "ready".into(),
            candidate: second,
            created_at: 10,
        });
    let mut value = good;
    let mut op = value["operations"][0].clone();
    op["id"] = json!("op2");
    op["candidates"] = json!(["second"]);
    op["dependencies"] = json!(["op1"]);
    value["operations"].as_array_mut().unwrap().push(op);
    let p: Proposal = serde_json::from_value(value)?;
    consolidation::validate(&p, &batch, std::slice::from_ref(&existing), &bundles)?;
    let r: Decisions = serde_json::from_value(
        json!({"schema_version":1,"role":"review","decisions":[{"operation":"op1","decision":"reject","rationale":"Unsupported generalization"},{"operation":"op2","decision":"accept","rationale":"Locally supported but dependent"}]}),
    )?;
    let record = notes::Review {
        decision: "accepted".into(),
        backend: "fixture".into(),
        model: "fixture".into(),
        prompt_version: "review-v1".into(),
        reviewed_at: 100,
    };
    let (changes, consumed) = consolidation::plan(
        &p,
        &r,
        &batch,
        std::slice::from_ref(&existing),
        &record,
        100,
    )?;
    assert!(changes.is_empty() && consumed.is_empty());
    Ok(())
}
struct Model {
    role: &'static str,
    decision: &'static str,
    calls: std::sync::atomic::AtomicUsize,
}
impl coding_memory::generation::Backend for Model {
    fn external(&self) -> bool {
        false
    }
    fn provenance(&self) -> serde_json::Value {
        json!({"backend":"fixture","model":self.role})
    }
    async fn generate(
        &self,
        r: coding_memory::generation::Request,
    ) -> std::result::Result<coding_memory::generation::Output, coding_memory::generation::Failure>
    {
        self.calls
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        assert!(r.attempt.starts_with(self.role));
        assert_eq!(r.schema["properties"]["role"]["enum"][0], self.role);
        let input: serde_json::Value =
            serde_json::from_str(r.prompt.split_once("INPUT_JSON\n").unwrap().1).unwrap();
        let output = if self.role == "consolidation" {
            let batch: coding_memory::consolidation_state::Batch =
                serde_json::from_value(input["batch"].clone()).unwrap();
            proposal(&batch, "create", None)
        } else {
            json!({"schema_version":1,"role":"review","decisions":[{"operation":"op1","decision":self.decision,"rationale":"Independent synthetic review"}]})
        };
        Ok(coding_memory::generation::Output {
            bytes: serde_json::to_vec(&output).unwrap(),
            thread_id: Some(format!("{}-thread", self.role)),
            runtime_version: "fixture".into(),
        })
    }
}
#[tokio::test]
async fn fresh_review_gates_the_complete_foreground_pipeline() -> Result<()> {
    for decision in ["accept", "reject", "hold", "approve"] {
        let d = tempdir()?;
        let root = d.path().join("state");
        let v = d.path().join("vault");
        fs::create_dir(&root)?;
        fs::create_dir(&v)?;
        let mut s = State::open(&root.join("state.sqlite"))?;
        s.register_project("alpha", "/repo", 1)?;
        candidate(&mut s, &root, 0, 10)?;
        let proposer = Model {
            role: "consolidation",
            decision,
            calls: 0.into(),
        };
        let reviewer = Model {
            role: "review",
            decision,
            calls: 0.into(),
        };
        let id = coding_memory::consolidation_worker::consolidate(
            &mut s, &root, &v, "alpha", true, &proposer, &reviewer,
        )
        .await?
        .unwrap();
        assert_eq!(proposer.calls.load(std::sync::atomic::Ordering::Relaxed), 1);
        assert_eq!(reviewer.calls.load(std::sync::atomic::Ordering::Relaxed), 1);
        assert_eq!(s.consolidation_status()?[0].id, id);
        assert_eq!(
            s.consolidation_status()?[0].state,
            if decision == "approve" {
                "held"
            } else {
                "applied"
            }
        );
        let files = fs::read_dir(&v)?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|e| e == "md"))
            .count();
        assert_eq!(files, usize::from(decision == "accept"));
        assert_eq!(
            s.candidates("alpha")?[0].state,
            if decision == "accept" {
                "consumed"
            } else {
                "held"
            }
        );
    }
    Ok(())
}
#[test]
fn five_candidates_trigger_before_age_and_other_projects_stay_separate() -> Result<()> {
    let d = tempdir()?;
    let mut s = State::open(&d.path().join("state.sqlite"))?;
    s.register_project("alpha", "/repo", 1)?;
    s.register_project("beta", "/other", 1)?;
    for n in 0..5 {
        candidate(&mut s, d.path(), n, 10)?;
    }
    assert!(s.reserve_batch("beta", 11, true)?.is_none());
    assert_eq!(
        s.reserve_batch("alpha", 11, false)?
            .unwrap()
            .candidates
            .len(),
        5
    );
    Ok(())
}
#[test]
fn semantic_related_links_are_rendered_from_supplied_paths_and_guarded() -> Result<()> {
    use coding_memory::consolidation::{self, Decisions, Proposal};
    let (_d, _s, batch, bundles, existing) = knowledge_fixture()?;
    let mut p = proposal(&batch, "create", None);
    p["operations"][0]["draft"]["related"] = json!(["queue-leases"]);
    let p: Proposal = serde_json::from_value(p)?;
    consolidation::validate(&p, &batch, std::slice::from_ref(&existing), &bundles)?;
    let r: Decisions = serde_json::from_value(
        json!({"schema_version":1,"role":"review","decisions":[{"operation":"op1","decision":"accept","rationale":"Clear synthetic relation"}]}),
    )?;
    let record = notes::Review {
        decision: "accepted".into(),
        backend: "fixture".into(),
        model: "fixture".into(),
        prompt_version: "review-v1".into(),
        reviewed_at: 100,
    };
    let (changes, _) = consolidation::plan(
        &p,
        &r,
        &batch,
        std::slice::from_ref(&existing),
        &record,
        100,
    )?;
    assert!(changes[0].content.contains("[[queue|queue-leases]]"));
    assert_eq!(changes.len(), 2);
    assert_eq!(
        changes[1].before.as_deref(),
        Some(changes[1].after.as_str())
    );
    Ok(())
}
#[test]
fn retention_deletes_only_old_unreferenced_no_value_evidence() -> Result<()> {
    let d = tempdir()?;
    let root = d.path().join("state");
    let v = d.path().join("vault");
    fs::create_dir(&root)?;
    fs::create_dir(&v)?;
    let mut s = State::open(&root.join("state.sqlite"))?;
    s.register_project("alpha", "/repo", 1)?;
    candidate(&mut s, &root, 0, 10)?;
    let kept = s.candidates("alpha")?[0].candidate.evidence[0]
        .bundle
        .clone();
    let mut bundle = evidence::load(&root, &kept)?;
    bundle.run.thread = "empty".into();
    let digest = evidence::save(&root, &bundle)?;
    s.enqueue(&bundle.run, "alpha", "/repo", 2)?;
    let c = s.claim(3, 10)?.unwrap();
    s.finish(&c, "completed", Some(&digest), 4)?;
    s.enqueue_extractions("alpha", 5)?;
    let c = s.claim_extraction("alpha", 6, 10)?.unwrap();
    s.finish_extraction(&c, &[], &json!({}), 10)?;
    let age = 30 * 86400;
    assert!(coding_memory::retention::cleanup(&mut s, &root, &v, age, 30, false)?.is_empty());
    assert_eq!(
        coding_memory::retention::cleanup(&mut s, &root, &v, age + 10, 30, false)?,
        vec![digest.clone()]
    );
    let mut note = notes::parse(include_str!("fixtures/vault/queue.md"))?;
    note.metadata.evidence = vec![notes::Reference {
        bundle: digest.clone(),
        item: "U".into(),
    }];
    fs::write(v.join("queue.md"), notes::render(&note)?)?;
    assert!(coding_memory::retention::cleanup(&mut s, &root, &v, age + 10, 30, true)?.is_empty());
    fs::remove_file(v.join("queue.md"))?;
    assert_eq!(
        coding_memory::retention::cleanup(&mut s, &root, &v, age + 10, 30, true)?,
        vec![digest.clone()]
    );
    assert!(evidence::load(&root, &digest).is_err());
    assert!(evidence::load(&root, &kept).is_ok());
    Ok(())
}
#[test]
fn edited_content_before_finalization_remains_a_conflict() -> Result<()> {
    let d = tempdir()?;
    let root = d.path().join("state");
    let v = d.path().join("vault");
    fs::create_dir(&root)?;
    fs::create_dir(&v)?;
    let mut s = State::open(&root.join("state.sqlite"))?;
    s.register_project("alpha", "/repo", 1)?;
    candidate(&mut s, &root, 0, 10)?;
    let batch = s.reserve_batch("alpha", 20, true)?.unwrap();
    let content = include_str!("fixtures/vault/queue.md").to_owned();
    s.save_plan(
        &batch,
        &v.canonicalize()?,
        &[Change {
            note_id: "queue-leases".into(),
            path: "queue-leases.md".into(),
            before: None,
            after: hash(content.as_bytes()),
            content,
        }],
        &[batch.candidates[0].id.clone()],
        &json!({}),
        21,
    )?;
    assert!(!vault::apply(&mut s, &root, &v, batch.id, |p| {
        if p == Point::BeforeFinalize {
            fs::write(v.join("queue-leases.md"), "concurrent edit")?;
        }
        Ok(())
    })?);
    assert_eq!(s.consolidation_status()?[0].state, "conflict");
    assert_eq!(s.candidates("alpha")?[0].state, "reserved");
    Ok(())
}
struct Disallowed;
impl coding_memory::generation::Backend for Disallowed {
    fn external(&self) -> bool {
        true
    }
    fn provenance(&self) -> serde_json::Value {
        json!({"backend":"fixture","model":"not-called"})
    }
    async fn generate(
        &self,
        _: coding_memory::generation::Request,
    ) -> std::result::Result<coding_memory::generation::Output, coding_memory::generation::Failure>
    {
        panic!("Disallowed transmission");
    }
}
#[tokio::test]
async fn empty_and_disallowed_batches_never_invoke_generation() -> Result<()> {
    let d = tempdir()?;
    let root = d.path().join("state");
    let v = d.path().join("vault");
    fs::create_dir(&root)?;
    fs::create_dir(&v)?;
    let mut s = State::open(&root.join("state.sqlite"))?;
    s.register_project("alpha", "/repo", 1)?;
    assert!(
        coding_memory::consolidation_worker::consolidate(
            &mut s,
            &root,
            &v,
            "alpha",
            true,
            &Disallowed,
            &Disallowed
        )
        .await?
        .is_none()
    );
    candidate(&mut s, &root, 0, 10)?;
    assert!(
        coding_memory::consolidation_worker::consolidate(
            &mut s,
            &root,
            &v,
            "alpha",
            true,
            &Disallowed,
            &Disallowed
        )
        .await?
        .is_some()
    );
    assert_eq!(
        s.consolidation_status()?[0].reason.as_deref(),
        Some("held_external_policy")
    );
    assert!(!root.join("generation").exists());
    Ok(())
}
#[test]
fn merged_claims_and_minor_updates_cannot_upgrade_verification() -> Result<()> {
    use coding_memory::consolidation::{self, Proposal};
    let (dir, _s, mut batch, mut bundles, existing) = knowledge_fixture()?;
    let mut observed_bundle = bundles.values().next().unwrap().clone();
    observed_bundle.items.push(json!({"id":"T","type":"commandExecution","status":"completed","exitCode":0,"aggregatedOutput":"PASS"}));
    let digest = evidence::save(dir.path(), &observed_bundle)?;
    bundles.insert(digest.clone(), observed_bundle);
    let mut observed = batch.candidates[0].candidate.clone();
    observed.verification = notes::Verification::Observed;
    observed.evidence = vec![notes::Reference {
        bundle: digest,
        item: "T".into(),
    }];
    batch
        .candidates
        .push(coding_memory::extraction_state::StagedCandidate {
            id: "observed".into(),
            state: "ready".into(),
            candidate: observed,
            created_at: 10,
        });
    let mut p = proposal(&batch, "create", None);
    p["operations"][0]["candidates"] = json!([batch.candidates[0].id, "observed"]);
    p["operations"][0]["draft"]["verification"] = json!("observed");
    p["operations"][0]["draft"]["evidence"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::to_value(
            &batch.candidates[1].candidate.evidence[0],
        )?);
    assert!(
        consolidation::validate(
            &serde_json::from_value::<Proposal>(p)?,
            &batch,
            std::slice::from_ref(&existing),
            &bundles
        )
        .is_err()
    );
    // All new claims are observed, but updating the existing user-stated note cannot upgrade it.
    batch.candidates.remove(0);
    let mut p = proposal(&batch, "update", Some("queue-leases"));
    p["operations"][0]["draft"]["evidence"]
        .as_array_mut()
        .unwrap()
        .extend(
            serde_json::to_value(notes::parse(&existing.markdown)?.metadata.evidence)?
                .as_array()
                .unwrap()
                .iter()
                .cloned(),
        );
    assert!(
        consolidation::validate(
            &serde_json::from_value::<Proposal>(p)?,
            &batch,
            std::slice::from_ref(&existing),
            &bundles
        )
        .is_err()
    );
    // Newly observed claims may form a successor; the predecessor retains its own category.
    let mut p = proposal(&batch, "supersede", Some("queue-leases"));
    p["operations"][0]["draft"]["summary"] =
        json!("A newly observed procedure under recorded conditions");
    consolidation::validate(
        &serde_json::from_value::<Proposal>(p)?,
        &batch,
        std::slice::from_ref(&existing),
        &bundles,
    )?;
    Ok(())
}
#[test]
fn exclusions_are_durable_when_no_batch_is_selected() -> Result<()> {
    for trigger_met in [true, false] {
        let d = tempdir()?;
        let mut s = State::open(&d.path().join("state.sqlite"))?;
        s.register_project("alpha", "/repo", 1)?;
        candidate(&mut s, d.path(), 0, 10)?;
        if !trigger_met {
            candidate(&mut s, d.path(), 1, 10)?;
        }
        s.exclude(
            &RunKey {
                store: "fixture".into(),
                thread: "t0".into(),
                turn: "r".into(),
            },
            "fixture",
        )?;
        assert!(s.reserve_batch("alpha", 11, trigger_met)?.is_none());
        drop(s);
        let s = State::open(&d.path().join("state.sqlite"))?;
        assert_eq!(
            s.candidates("alpha")?
                .iter()
                .filter(|c| c.state == "held")
                .count(),
            1
        );
    }
    Ok(())
}
#[test]
fn self_related_updates_are_rejected_during_proposal_validation() -> Result<()> {
    use coding_memory::consolidation::{self, Proposal};
    let (_d, _s, batch, bundles, existing) = knowledge_fixture()?;
    let mut p = proposal(&batch, "update", Some("queue-leases"));
    p["operations"][0]["draft"]["related"] = json!(["queue-leases"]);
    assert!(
        consolidation::validate(
            &serde_json::from_value::<Proposal>(p)?,
            &batch,
            std::slice::from_ref(&existing),
            &bundles
        )
        .is_err()
    );
    Ok(())
}

struct NeighborModel {
    role: &'static str,
    action: &'static str,
}
impl coding_memory::generation::Backend for NeighborModel {
    fn external(&self) -> bool {
        false
    }
    fn provenance(&self) -> serde_json::Value {
        json!({"backend":"fixture","model":self.role})
    }
    async fn generate(
        &self,
        r: coding_memory::generation::Request,
    ) -> std::result::Result<coding_memory::generation::Output, coding_memory::generation::Failure>
    {
        let input: serde_json::Value =
            serde_json::from_str(r.prompt.split_once("INPUT_JSON\n").unwrap().1).unwrap();
        let context = if self.role == "review" {
            &input["context"]
        } else {
            &input
        };
        assert_eq!(context["existing"].as_array().unwrap().len(), 1);
        assert_eq!(context["existing"][0]["id"], "queue-leases");
        let output = if self.role == "review" {
            json!({"schema_version":1,"role":"review","decisions":[{"operation":"op1","decision":"accept","rationale":"Synthetic support and applicability preserved"}]})
        } else {
            let batch = serde_json::from_value(context["batch"].clone()).unwrap();
            let mut p = proposal(
                &batch,
                self.action,
                if self.action == "create" {
                    None
                } else {
                    Some("queue-leases")
                },
            );
            if self.action == "skip" {
                p["operations"][0]["draft"] = json!(null);
            }
            if self.action == "update" {
                let old =
                    notes::parse(context["existing"][0]["markdown"].as_str().unwrap()).unwrap();
                let refs = p["operations"][0]["draft"]["evidence"]
                    .as_array_mut()
                    .unwrap();
                for reference in old.metadata.evidence {
                    let reference = serde_json::to_value(reference).unwrap();
                    if !refs.contains(&reference) {
                        refs.push(reference);
                    }
                }
            }
            if self.action == "create" {
                p["operations"][0]["draft"]["related"] = json!(["queue-leases"]);
            }
            p
        };
        Ok(coding_memory::generation::Output {
            bytes: serde_json::to_vec(&output).unwrap(),
            thread_id: Some(self.role.into()),
            runtime_version: "fixture".into(),
        })
    }
}
#[tokio::test]
async fn retrieved_paraphrases_reach_both_roles_for_update_skip_and_distinct_related_creation()
-> Result<()> {
    for action in ["update", "skip", "create"] {
        let d = tempdir()?;
        let vault = d.path().join("vault");
        fs::create_dir(&vault)?;
        let mut s = State::open(&d.path().join("state.sqlite"))?;
        s.register_project("alpha", "/repo", 1)?;
        candidate_with_text(
            &mut s,
            d.path(),
            0,
            10,
            if action == "create" {
                "Track failures for queued jobs"
            } else {
                "Use leased jobs"
            },
            if action == "create" {
                "Record job failures separately."
            } else {
                "Use leased jobs."
            },
        )?;
        let c = s.candidates("alpha")?[0].candidate.clone();
        let mut old = notes::parse(include_str!("fixtures/vault/queue.md"))?;
        old.metadata.title = "Recover token-fenced jobs".into();
        old.metadata.summary = "Allocate jobs with leases.".into();
        old.body = old.metadata.summary.clone();
        old.metadata.conditions = c.conditions.clone();
        let mut old_bundle = evidence::load(d.path(), &c.evidence[0].bundle)?;
        old_bundle.run.thread = "existing-support".into();
        old_bundle.items =
            vec![json!({"id":"U","type":"userMessage","text":"Allocate jobs with leases."})];
        old.metadata.evidence = vec![notes::Reference {
            bundle: evidence::save(d.path(), &old_bundle)?,
            item: "U".into(),
        }];
        let old_refs = old.metadata.evidence.clone();
        old.metadata.verification = c.verification.clone();
        fs::write(vault.join("queue.md"), notes::render(&old)?)?;
        let search = Search::new(d.path(), &vault)?;
        search.rebuild()?;
        assert!(
            search
                .search(&c.title, &Scope::new("alpha"), 3)?
                .results
                .is_empty()
        );
        let id = coding_memory::consolidation_worker::consolidate(
            &mut s,
            d.path(),
            &vault,
            "alpha",
            true,
            &NeighborModel {
                role: "consolidation",
                action,
            },
            &NeighborModel {
                role: "review",
                action,
            },
        )
        .await?
        .unwrap();
        assert_eq!(
            s.consolidation_status()?
                .iter()
                .find(|j| j.id == id)
                .unwrap()
                .state,
            "applied"
        );
        assert_eq!(s.candidates("alpha")?[0].state, "consumed");
        let files: Vec<_> = fs::read_dir(&vault)?.map(|e| e.unwrap().path()).collect();
        assert_eq!(files.len(), if action == "create" { 2 } else { 1 });
        if action == "create" {
            assert!(files.iter().any(|p| {
                fs::read_to_string(p)
                    .unwrap()
                    .contains("[[queue|queue-leases]]")
            }));
        }
        let stored = notes::parse(&fs::read_to_string(vault.join("queue.md"))?)?;
        assert_eq!(stored.metadata.conditions, c.conditions);
        assert_eq!(stored.metadata.verification, c.verification);
        assert!(
            old_refs
                .iter()
                .all(|e| stored.metadata.evidence.contains(e))
        );
        if action == "update" {
            assert!(
                c.evidence
                    .iter()
                    .all(|e| stored.metadata.evidence.contains(e))
            );
        }
    }
    Ok(())
}
