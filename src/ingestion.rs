use crate::{
    codex::{self, Capture, Client},
    domain::{RunKey, hash},
    dsh, evidence, project,
    state::{Enqueue, State},
};
use anyhow::{Result, ensure};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock after Unix epoch")
        .as_secs() as i64
}
pub fn store_id(home: &Path) -> Result<String> {
    Ok(hash(
        home.canonicalize()?
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("Non-UTF8 runtime store"))?
            .as_bytes(),
    ))
}
fn identity(value: &Value, field: &str) -> Result<String> {
    let id = value[field]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Missing {field}"))?;
    ensure!(!id.is_empty() && id.len() <= 256, "Invalid identity");
    Ok(id.into())
}
pub fn signal(value: &Value, store: &str) -> Result<(RunKey, String)> {
    let (thread, turn) = if value["type"] == "agent-turn-complete" {
        ("thread-id", "turn-id")
    } else {
        ensure!(
            ["Stop", "Interrupt"].contains(&value["hook_event_name"].as_str().unwrap_or("")),
            "Unsupported capture signal"
        );
        ("session_id", "turn_id")
    };
    Ok((
        RunKey {
            store: store.into(),
            thread: identity(value, thread)?,
            turn: identity(value, turn)?,
        },
        value["cwd"]
            .as_str()
            .filter(|s| !s.is_empty() && s.len() <= 4096)
            .ok_or_else(|| anyhow::anyhow!("Missing or invalid cwd"))?
            .into(),
    ))
}
pub fn enqueue_signal(state: &mut State, home: &Path, value: &Value) -> Result<Enqueue> {
    let (key, cwd) = signal(value, &store_id(home)?)?;
    let p = project::resolve(Path::new(&cwd))?;
    state.project_epoch(&p.id)?; // Explicit registration is mandatory.
    state.enqueue(&key, &p.id, &cwd, now())
}
/// A complete scoped scan, not a fragile updated-at watermark. Cursors are per scan.
pub async fn discover(
    client: &mut Client,
    home: &Path,
    path: &Path,
) -> Result<Vec<(RunKey, Value)>> {
    let mut roots = project::worktrees(path)?;
    let cwd = path.canonicalize()?;
    if !roots.contains(&cwd) {
        roots.push(cwd);
    }
    let store = store_id(home)?;
    let mut result = Vec::new();
    let mut bytes = 0;
    for thread in client.threads(&roots).await? {
        let tid = identity(&thread, "id")?;
        if !thread["parentThreadId"].is_null() {
            continue;
        }
        for turn in client.turns(&tid).await? {
            bytes += serde_json::to_vec(&turn)?.len();
            ensure!(bytes <= codex::MAX_BYTES, "Discovery exceeds capture limit");
            result.push((
                RunKey {
                    store: store.clone(),
                    thread: tid.clone(),
                    turn: identity(&turn, "id")?,
                },
                turn,
            ));
        }
    }
    Ok(result)
}
pub async fn register(
    state: &mut State,
    client: &mut Client,
    home: &Path,
    path: &Path,
) -> Result<String> {
    let p = project::resolve(path)?;
    if state.project_epoch(&p.id).is_ok() {
        return Ok(p.id);
    }
    let epoch = now();
    let baseline = discover(client, home, path).await?;
    // A crash during baseline insertion is safe: pre-epoch runs remain ineligible.
    state.register_project(
        &p.id,
        p.repository
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("Non-UTF8 path"))?,
        epoch,
    )?;
    for (key, _) in baseline {
        state.exclude(&key, "enablement_baseline")?;
    }
    Ok(p.id)
}
pub async fn reconcile(
    state: &mut State,
    client: &mut Client,
    home: &Path,
    path: &Path,
) -> Result<usize> {
    let audit = state.observe_runtime(client.identity(), now())?;
    ensure!(audit.validation != "failed", "runtime_validation_failed");
    let p = project::resolve(path)?;
    let epoch = state.project_epoch(&p.id)?;
    let mut count = 0;
    let mut metadata = HashMap::new();
    for (key, turn) in discover(client, home, path).await? {
        if turn["startedAt"]
            .as_i64()
            .is_none_or(|started| started <= epoch)
        {
            state.exclude(&key, "before_enablement_or_unknown_start")?;
            continue;
        }
        if !metadata.contains_key(&key.thread) {
            let response = client
                .call(
                    "thread/read",
                    json!({"threadId":key.thread,"includeTurns":false}),
                )
                .await?;
            metadata.insert(key.thread.clone(), response["thread"].clone());
        }
        let thread = &metadata[&key.thread];
        if let Ok(Some(cwd)) = codex::turn_cwd(home, thread, &key.turn)
            && project::resolve(&cwd)?.id != p.id
        {
            continue;
        }
        // Capture repeats association validation. Missing/invalid source metadata
        // remains inspectable instead of being accepted from the thread cwd.
        if state.enqueue(
            &key,
            &p.id,
            path.to_str()
                .ok_or_else(|| anyhow::anyhow!("Non-UTF8 path"))?,
            now(),
        )? == Enqueue::Inserted
        {
            count += 1;
        }
    }
    Ok(count)
}
pub async fn capture_one(
    state: &mut State,
    client: &mut Client,
    home: &Path,
    root: &Path,
) -> Result<bool> {
    let identity = client.identity().clone();
    let audit = state.observe_runtime(&identity, now())?;
    ensure!(audit.validation != "failed", "runtime_validation_failed");
    let store = store_id(home)?;
    let Some(claim) = state.claim_store(Some(&store), now(), 3600)? else {
        return Ok(false);
    };
    if claim.key.store != store_id(home)? {
        state.hold(&claim, "wrong_runtime_store", now())?;
        return Ok(true);
    }
    let result = codex::capture(client, home, &claim.key, &claim.project).await;
    match result {
        Ok(Capture::Ready(mut bundle)) => {
            let epoch = state.project_epoch(&claim.project)?;
            if bundle.started_at <= epoch {
                state.hold(&claim, "before_enablement_or_unknown_start", now())?;
                return Ok(true);
            }
            let tx = state
                .db
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let published_at = now();
            let audit = crate::compatibility::observe_runtime_on(&tx, &identity, published_at)?;
            if audit.validation == "failed" {
                State::transition_on(
                    &tx,
                    &claim,
                    "held",
                    Some("runtime_validation_failed"),
                    None,
                    None,
                    published_at,
                    published_at,
                )?;
            } else {
                bundle.capture_runtime = Some(audit);
                let digest = evidence::save(root, &bundle)?;
                let outcome_state = if bundle.outcome == "completed" {
                    "ready"
                } else {
                    "recorded"
                };
                let finished_at = now();
                State::transition_on(
                    &tx,
                    &claim,
                    outcome_state,
                    None,
                    Some(&digest),
                    Some(&bundle.outcome),
                    finished_at,
                    finished_at,
                )?;
            }
            tx.commit()?;
        }
        Ok(Capture::Pending) if claim.attempts >= 60 => {
            state.hold(&claim, "history_readiness_exhausted", now())?
        }
        Ok(Capture::Pending) => state.defer(&claim, "history_pending", now(), 10)?,
        Ok(Capture::Missing) => state.hold(&claim, "history_missing", now())?,
        Ok(Capture::Unsupported(reason)) => state.hold(&claim, reason.reason(), now())?,
        Ok(Capture::OverLimit) => state.hold(&claim, "history_over_limit", now())?,
        Ok(Capture::Excluded) => state.hold(&claim, "excluded_inherited_or_nonhuman", now())?,
        Ok(Capture::WrongProject) => state.hold(&claim, "project_mismatch", now())?,
        Err(error) => state.hold(
            &claim,
            match error.downcast_ref::<codex::ReadFailure>() {
                Some(codex::ReadFailure::Missing) => "history_missing",
                Some(codex::ReadFailure::Unsupported(reason)) => reason.reason(),
                Some(codex::ReadFailure::OverLimit) => "history_over_limit",
                None => "reader_failure",
            },
            now(),
        )?,
    }
    Ok(true)
}

/// A DeepSeek Harness capture request from the bridge.
///
/// The bridge is a hint source, like a Codex notification: it names a turn and
/// the projection file to read, and the engine alone decides whether that turn
/// becomes evidence.
#[derive(Debug, Clone)]
pub enum DshOperation {
    Enqueue,
    Capture {
        session: PathBuf,
        project: PathBuf,
        turn: Option<u64>,
    },
    Reconcile {
        session: PathBuf,
        project: PathBuf,
    },
    Register {
        session: PathBuf,
        project: PathBuf,
    },
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DshReport {
    pub action: &'static str,
    /// State reached by the run this invocation handled, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run: Option<RunKey>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evidence: Option<String>,
    /// Runs queued by `reconcile`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub queued: Option<usize>,
    /// Turns excluded by `register` as the enablement baseline.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub baseline: Option<usize>,
}

/// Execute one bridge request.
pub async fn dsh(
    state: &mut State,
    store: &str,
    root: &Path,
    operation: &DshOperation,
    event: Option<String>,
) -> Result<DshReport> {
    match operation {
        DshOperation::Enqueue => {
            let mut bytes = Vec::new();
            if let Some(event) = event {
                bytes = event.into_bytes();
            } else {
                use std::io::Read;
                std::io::stdin().take(65537).read_to_end(&mut bytes)?;
            }
            ensure!(bytes.len() <= 65536, "Signal exceeds size limit");
            let (key, cwd) = dsh::signal(&serde_json::from_slice(&bytes)?, store)?;
            let p = project::resolve(Path::new(&cwd))?;
            if state.project_epoch(&p.id).is_err() {
                // A live run can end before its project is registered. Keep the
                // exact hint and let a later reconcile resolve it; silently
                // dropping it would lose the only record that this turn ended.
                state.save_pending_hint(&key, &cwd, now())?;
                return Ok(DshReport {
                    action: "enqueue",
                    state: Some("deferred"),
                    run: Some(key),
                    reason: Some("project_not_registered".into()),
                    evidence: None,
                    queued: None,
                    baseline: None,
                });
            }
            let queued = state.enqueue_hinted(&key, &p.id, &cwd, now())?;
            Ok(DshReport {
                action: "enqueue",
                state: Some(match queued {
                    Enqueue::Inserted => "queued",
                    Enqueue::Duplicate => "duplicate",
                    Enqueue::Excluded => "held",
                }),
                run: Some(key),
                reason: (queued == Enqueue::Excluded).then(|| "excluded".into()),
                evidence: None,
                queued: None,
                baseline: None,
            })
        }
        DshOperation::Register { session, project } => {
            let p = project::resolve(project)?;
            let cwd = session_cwd(&p, session)?;
            if state.project_epoch(&p.id).is_ok() {
                // Registration is a one-time enablement boundary. Re-running it
                // must not baseline later turns: those are valid capture work
                // that a queued run or a retry may still own.
                return Ok(DshReport {
                    action: "register",
                    state: None,
                    run: None,
                    reason: Some("already_registered".into()),
                    evidence: None,
                    queued: None,
                    baseline: Some(0),
                });
            }
            let epoch = now();
            let baseline = dsh::discover(session, &cwd, store)?;
            state.register_project(
                &p.id,
                p.repository
                    .to_str()
                    .ok_or_else(|| anyhow::anyhow!("Non-UTF8 path"))?,
                epoch,
            )?;
            for run in &baseline {
                state.exclude(&run.turn, "enablement_baseline")?;
            }
            Ok(DshReport {
                action: "register",
                state: None,
                run: None,
                reason: None,
                evidence: None,
                queued: None,
                baseline: Some(baseline.len()),
            })
        }
        DshOperation::Reconcile { session, project } => {
            let p = project::resolve(project)?;
            let cwd = session_cwd(&p, session)?;
            let epoch = state.project_epoch(&p.id)?;
            let mut queued = 0;
            let projection = dsh::Projection::parse(&evidence::read_bounded(
                session,
                evidence::MAX_SOURCE_BYTES,
            )?)?;
            // Only this projection's session: another session's hint must stay
            // pending until its own projection is reconciled, not be resolved
            // (or dropped as `history_missing`) against this one.
            let hinted: Vec<RunKey> = state
                .pending_hints(store)?
                .into_iter()
                .filter(|(key, hint_cwd)| {
                    key.thread == projection.session()
                        && project::resolve(Path::new(hint_cwd)).is_ok_and(|h| h.id == p.id)
                })
                .map(|(key, _)| key)
                .collect();
            ensure!(
                projection.cwd() == Some(cwd.as_str()),
                "Projection recorded a different project directory"
            );
            // A hint is authoritative: the engine only captures a run some live
            // process saw end. Each hint resolves to its exact turn; a run that
            // ended before enablement is excluded exactly like a baseline turn.
            let runs = projection.completed_runs();
            for hint in hinted {
                let Some(turn) = runs.iter().find(|run| run.turn.to_string() == hint.turn) else {
                    state.hold_hint(&hint, "history_missing")?;
                    continue;
                };
                // `ended_at` is milliseconds; the epoch is a second.
                if turn.ended_at <= epoch.saturating_mul(1000) {
                    state.exclude(&hint, "before_enablement")?;
                    state.resolve_hint(&hint)?;
                    continue;
                }
                if state.enqueue_hinted(&hint, &p.id, &cwd, now())? == Enqueue::Inserted {
                    queued += 1;
                }
                state.resolve_hint(&hint)?;
            }
            Ok(DshReport {
                action: "reconcile",
                state: None,
                run: None,
                reason: None,
                evidence: None,
                queued: Some(queued),
                baseline: None,
            })
        }
        DshOperation::Capture {
            session,
            project,
            turn,
        } => {
            let projection = dsh::Projection::parse(&evidence::read_bounded(
                session,
                evidence::MAX_SOURCE_BYTES,
            )?)?;
            // Claim first, so a run that cannot be captured is held with a
            // reason instead of staying queued and being selected forever.
            let Some(claim) =
                state.claim_session(Some(store), projection.session(), now(), 3600)?
            else {
                return Ok(DshReport {
                    action: "capture",
                    state: None,
                    run: None,
                    reason: Some("no_ready_run".into()),
                    evidence: None,
                    queued: None,
                    baseline: None,
                });
            };
            let (p, cwd) = match project::resolve(project)
                .and_then(|p| session_cwd(&p, session).map(|cwd| (p, cwd)))
            {
                Ok(resolved) => resolved,
                Err(_) => {
                    // The recorded directory was deleted, moved, or now resolves
                    // to another repository: hold it, do not re-queue it.
                    state.hold(&claim, "project_mismatch", now())?;
                    return Ok(DshReport {
                        action: "capture",
                        state: Some("held"),
                        run: Some(claim.key),
                        reason: Some("project_mismatch".into()),
                        evidence: None,
                        queued: None,
                        baseline: None,
                    });
                }
            };
            if claim.key.thread != projection.session() {
                state.hold(&claim, "projection_session_mismatch", now())?;
                return Ok(DshReport {
                    action: "capture",
                    state: Some("held"),
                    run: Some(claim.key),
                    reason: Some("projection_session_mismatch".into()),
                    evidence: None,
                    queued: None,
                    baseline: None,
                });
            }
            if !state.hinted(&claim.key)? {
                // Only a hint can name the turned-ended run this process saw.
                // Without one, the projection belongs to a different process
                // or turn and must not be published as this run's evidence.
                state.hold(&claim, "no_capture_hint", now())?;
                return Ok(DshReport {
                    action: "capture",
                    state: Some("held"),
                    run: Some(claim.key),
                    reason: Some("no_capture_hint".into()),
                    evidence: None,
                    queued: None,
                    baseline: None,
                });
            }
            if claim.key.store != store || claim.project != p.id {
                state.hold(&claim, "wrong_runtime_store", now())?;
                return Ok(DshReport {
                    action: "capture",
                    state: Some("held"),
                    run: Some(claim.key),
                    reason: Some("wrong_runtime_store".into()),
                    evidence: None,
                    queued: None,
                    baseline: None,
                });
            }
            // An explicit turn may only name the run already claimed for this
            // store, so a stale bridge hint cannot capture a different run than
            // the one the queue selected.
            if let Some(turn) = turn {
                ensure!(
                    claim.key.turn == turn.to_string(),
                    "Requested turn is not the claimed run"
                );
            }
            let turn_number: u64 = claim.key.turn.parse()?;
            let epoch = state.project_epoch(&claim.project)?;
            let document = serde_json::to_value(&projection)?;
            let outcome = dsh::decode(&document, store, &cwd, turn_number, epoch)?;
            dsh_capture(state, root, &claim, outcome)
        }
    }
}

/// The cwd a projection recorded, which must belong to the registered project:
/// a session cannot be attributed to a project it did not run in.
fn session_cwd(p: &crate::project::Project, session: &Path) -> Result<String> {
    let document: serde_json::Value = serde_json::from_slice(&evidence::read_bounded(
        session,
        evidence::MAX_SOURCE_BYTES,
    )?)?;
    let cwd = document["header"]["cwd"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Projection recorded no cwd"))?;
    let recorded = project::resolve(Path::new(cwd))?;
    ensure!(recorded.id == p.id, "project_mismatch");
    Ok(cwd.into())
}

/// Publish one decoded DSH turn with the same transitions Codex capture uses.
fn dsh_capture(
    state: &mut State,
    root: &Path,
    claim: &crate::state::Claim,
    outcome: dsh::Capture,
) -> Result<DshReport> {
    use dsh::Capture;
    let mut report = DshReport {
        action: "capture",
        state: None,
        run: Some(claim.key.clone()),
        reason: None,
        evidence: None,
        queued: None,
        baseline: None,
    };
    match outcome {
        Capture::Ready(bundle) => {
            let mut bundle = *bundle;
            bundle.project = claim.project.clone();
            let digest = evidence::save(root, &bundle)?;
            let state_name = if bundle.outcome == "completed" {
                "ready"
            } else {
                "recorded"
            };
            state.finish(claim, &bundle.outcome, Some(&digest), now())?;
            report.state = Some(state_name);
            report.evidence = Some(digest);
        }
        Capture::Pending => {
            state.defer(claim, "history_pending", now(), 10)?;
            report.state = Some("queued");
            report.reason = Some("history_pending".into());
        }
        Capture::Missing => {
            state.hold(claim, "history_missing", now())?;
            report.state = Some("held");
            report.reason = Some("history_missing".into());
        }
        Capture::Unsupported(reason) => {
            state.hold(claim, reason.reason(), now())?;
            report.state = Some("held");
            report.reason = Some(reason.reason().into());
        }
        Capture::OverLimit => {
            state.hold(claim, "history_over_limit", now())?;
            report.state = Some("held");
            report.reason = Some("history_over_limit".into());
        }
        Capture::BeforeEnablement => {
            // Reported separately: an operator diagnosing a missing capture must
            // not be told a historical run was a fork or a nonhuman run.
            state.hold(claim, "before_enablement", now())?;
            report.state = Some("held");
            report.reason = Some("before_enablement".into());
        }
        Capture::Excluded => {
            state.hold(claim, "excluded_inherited_or_nonhuman", now())?;
            report.state = Some("held");
            report.reason = Some("excluded_inherited_or_nonhuman".into());
        }
        Capture::WrongProject => {
            state.hold(claim, "project_mismatch", now())?;
            report.state = Some("held");
            report.reason = Some("project_mismatch".into());
        }
    }
    Ok(report)
}
