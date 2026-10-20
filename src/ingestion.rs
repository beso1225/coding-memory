use crate::{
    clock,
    codex::{self, Capture, Client},
    domain::{RunKey, hash},
    dsh, evidence, project,
    state::{Claim, Enqueue, State},
};
use anyhow::{Result, ensure};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

pub use crate::clock::{now, now_ms};

/// Identity of a Codex runtime store.
///
/// The historical path-only identity is kept so state written before other
/// runtimes existed stays valid.
pub fn store_id(home: &Path) -> Result<String> {
    Ok(hash(
        home.canonicalize()?
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("Non-UTF8 runtime store"))?
            .as_bytes(),
    ))
}
/// Identity of a DeepSeek Harness runtime store.
///
/// Runtime-qualified, so a `--dsh-home` that happens to resolve to a configured
/// Codex home can never lease, exclude, or publish the other runtime's runs.
pub fn dsh_store_id(home: &Path) -> Result<String> {
    let path = home.canonicalize()?;
    let path = path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("Non-UTF8 runtime store"))?;
    Ok(hash(format!("dsh\0{path}").as_bytes()))
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
    // Choose the boundary before the baseline scan: the scan can take longer
    // than a second, and a run that ends during it must not be treated as
    // pre-enablement by a later clock sample.
    let enabled_ms = now_ms();
    let epoch = enabled_ms / 1000;
    let baseline = discover(client, home, path).await?;
    // A crash during baseline insertion is safe: pre-epoch runs remain ineligible.
    // A concurrent registration that lost the insert race reports its own
    // return value here; it must not baseline under its later local instant.
    if !state.register_project_at(
        &p.id,
        p.repository
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("Non-UTF8 path"))?,
        epoch,
        enabled_ms,
    )? {
        return Ok(p.id);
    }
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
    /// Record one bounded `turn-end` signal, from the argument or stdin.
    Enqueue {
        signal: Option<String>,
    },
    Capture {
        session: PathBuf,
        project: PathBuf,
        turn: Option<u64>,
        /// Session identity from the bridge, trusted only to locate the run.
        ///
        /// It lets capture claim and hold a run even when the projection is
        /// unreadable; the parsed projection is still checked against it.
        thread: Option<String>,
    },
    Reconcile {
        session: PathBuf,
        project: PathBuf,
    },
    Register {
        session: PathBuf,
        project: PathBuf,
    },
    /// Mark this DSH store as engine-owned, like Codex `own-store`.
    OwnStore,
    /// Hold one queued run whose projection cannot be transported.
    ///
    /// The bridge calls this after deciding a turn exceeds its own projection
    /// bound; the run would otherwise stay queued with no capture driver.
    HoldOverLimit {
        thread: String,
        turn: u64,
    },
}

/// Stable reason codes a DSH invocation can report or record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reason {
    NoReadyRun,
    ProjectNotRegistered,
    AlreadyRegistered,
    Excluded,
    UnsupportedProjectionSchema,
    ReaderFailure,
    ProjectMismatch,
    ProjectionSessionMismatch,
    NoCaptureHint,
    InvalidTurnIdentity,
    HistoryPending,
    HistoryMissing,
    HistoryReadinessExhausted,
    HistoryOverLimit,
    BeforeEnablement,
    ExcludedInheritedOrNonhuman,
}
impl Reason {
    fn as_str(self) -> &'static str {
        match self {
            Self::NoReadyRun => "no_ready_run",
            Self::ProjectNotRegistered => "project_not_registered",
            Self::AlreadyRegistered => "already_registered",
            Self::Excluded => "excluded",
            Self::UnsupportedProjectionSchema => "unsupported_projection_schema",
            Self::ReaderFailure => "reader_failure",
            Self::ProjectMismatch => "project_mismatch",
            Self::ProjectionSessionMismatch => "projection_session_mismatch",
            Self::NoCaptureHint => "no_capture_hint",
            Self::InvalidTurnIdentity => "invalid_turn_identity",
            Self::HistoryPending => "history_pending",
            Self::HistoryMissing => "history_missing",
            Self::HistoryReadinessExhausted => "history_readiness_exhausted",
            Self::HistoryOverLimit => "history_over_limit",
            Self::BeforeEnablement => "before_enablement",
            Self::ExcludedInheritedOrNonhuman => "excluded_inherited_or_nonhuman",
        }
    }
}

/// Attempts one capture may defer before the run becomes inspectable.
///
/// The bridge's `CAPTURE_ATTEMPTS` must equal this value; a cross-language test
/// compares them, because the bridge is the only driver of a DSH run.
pub const DSH_PENDING_ATTEMPT_LIMIT: i64 = 60;
/// Report reasons after which the bridge can retry without a new hint.
pub const DSH_RETRYABLE_REASONS: &[&str] = &["history_pending", "no_ready_run"];
/// Lease held by one capture attempt.
///
/// It is deliberately short: a crashed capture must become claimable again
/// within the bridge's `no_ready_run` retry window (it retries every ten
/// seconds, so this leaves several attempts to recover the run).
const CAPTURE_LEASE: i64 = 60;
/// Pending hints older than this are dropped during reconciliation.
const PENDING_HINT_TTL: i64 = 30 * 24 * 60 * 60;

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
impl DshReport {
    fn new(action: &'static str) -> Self {
        Self {
            action,
            state: None,
            run: None,
            reason: None,
            evidence: None,
            queued: None,
            baseline: None,
        }
    }
    fn state(mut self, state: &'static str) -> Self {
        self.state = Some(state);
        self
    }
    fn run(mut self, run: RunKey) -> Self {
        self.run = Some(run);
        self
    }
    fn reason(self, reason: Reason) -> Self {
        self.reason_str(reason.as_str())
    }
    fn reason_str(mut self, reason: &'static str) -> Self {
        self.reason = Some(reason.into());
        self
    }
    fn evidence(mut self, digest: String) -> Self {
        self.evidence = Some(digest);
        self
    }
    fn queued(mut self, queued: usize) -> Self {
        self.queued = Some(queued);
        self
    }
    fn baseline(mut self, baseline: usize) -> Self {
        self.baseline = Some(baseline);
        self
    }
}

/// Execute one bridge request.
pub fn dsh(
    state: &mut State,
    store: &str,
    root: &Path,
    operation: DshOperation,
) -> Result<DshReport> {
    match operation {
        DshOperation::OwnStore => {
            state.own_store(store)?;
            Ok(DshReport::new("own-store"))
        }
        DshOperation::HoldOverLimit { thread, turn } => {
            dsh_hold_over_limit(state, store, &thread, turn)
        }
        DshOperation::Enqueue { signal } => dsh_enqueue(state, store, signal),
        DshOperation::Register { session, project } => {
            dsh_register(state, store, &session, &project)
        }
        DshOperation::Reconcile { session, project } => {
            dsh_reconcile(state, store, &session, &project)
        }
        DshOperation::Capture {
            session,
            project,
            turn,
            thread,
        } => dsh_capture_turn(
            state,
            store,
            root,
            &session,
            &project,
            turn,
            thread.as_deref(),
        ),
    }
}

fn dsh_hold_over_limit(
    state: &mut State,
    store: &str,
    thread: &str,
    turn: u64,
) -> Result<DshReport> {
    let key = RunKey {
        store: store.into(),
        thread: thread.into(),
        turn: turn.to_string(),
    };
    if !state.hold_run(&key, Reason::HistoryOverLimit.as_str(), clock::now())? {
        return Ok(DshReport::new("hold").reason(Reason::NoReadyRun));
    }
    Ok(DshReport::new("hold")
        .state("held")
        .run(key)
        .reason(Reason::HistoryOverLimit))
}

fn dsh_enqueue(state: &mut State, store: &str, signal: Option<String>) -> Result<DshReport> {
    let bytes = match signal {
        Some(signal) => signal.into_bytes(),
        None => read_signal()?,
    };
    ensure!(
        bytes.len() <= dsh::MAX_SIGNAL_BYTES,
        "Signal exceeds size limit"
    );
    let (key, cwd) = dsh::signal(&serde_json::from_slice(&bytes)?, store)?;
    let p = project::resolve(Path::new(&cwd))?;
    let report = DshReport::new("enqueue").run(key.clone());
    if state.project_enabled(&p.id)?.is_none() {
        // A live run can end before its project is registered. Keep the exact
        // hint and let a later reconcile resolve it; silently dropping it would
        // lose the only record that this turn ended.
        state.save_pending_hint(&key, &cwd, clock::now())?;
        return Ok(report
            .state("deferred")
            .reason(Reason::ProjectNotRegistered));
    }
    let queued = state.enqueue_hinted(&key, &p.id, &cwd, clock::now())?;
    Ok(match queued {
        Enqueue::Inserted => report.state("queued"),
        Enqueue::Duplicate => report.state("duplicate"),
        Enqueue::Excluded => report.state("held").reason(Reason::Excluded),
    })
}

fn dsh_register(
    state: &mut State,
    store: &str,
    session: &Path,
    project: &Path,
) -> Result<DshReport> {
    let p = project::resolve(project)?;
    let report = DshReport::new("register");
    if state.project_enabled(&p.id)?.is_some() {
        // Registration is a one-time enablement boundary. Re-running it must
        // not baseline later turns: those are valid capture work that a queued
        // run or a retry may still own.
        return Ok(report.reason(Reason::AlreadyRegistered).baseline(0));
    }
    let projection = dsh::Projection::parse(&evidence::read_bounded(
        session,
        evidence::MAX_SOURCE_BYTES,
    )?)?;
    let cwd = projection_cwd(&p, &projection)?;
    // The baseline scan below can take longer than a second; the boundary is
    // the instant chosen before it, not a later sample.
    let enabled_ms = clock::now_ms();
    let epoch = enabled_ms / 1000;
    let baseline = dsh::discover(&projection, &cwd, store)?;
    if !state.register_project_at(
        &p.id,
        p.repository
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("Non-UTF8 path"))?,
        epoch,
        enabled_ms,
    )? {
        // Another registration won the insert race. Its stored instant is
        // authoritative, so this process must not apply a baseline.
        return Ok(report.reason(Reason::AlreadyRegistered).baseline(0));
    }
    // Only turns that ended at or before the chosen instant are the baseline. A
    // turn that ends while this scan runs is post-enablement work with a live
    // hint, and excluding it would discard a valid run.
    let mut baselined = 0;
    for run in &baseline {
        if run.ended_at <= enabled_ms {
            state.exclude(&run.turn, "enablement_baseline")?;
            baselined += 1;
        }
    }
    Ok(report.baseline(baselined))
}

fn dsh_reconcile(
    state: &mut State,
    store: &str,
    session: &Path,
    project: &Path,
) -> Result<DshReport> {
    let p = project::resolve(project)?;
    state.expire_pending_hints(store, clock::now() - PENDING_HINT_TTL)?;
    let projection = dsh::Projection::parse(&evidence::read_bounded(
        session,
        evidence::MAX_SOURCE_BYTES,
    )?)?;
    // Attribution comes from this parsed document, not a second read.
    let cwd = projection_cwd(&p, &projection)?;
    let boundary = project_boundary(state, &p.id)?;
    // Only this projection's session: another session's hint must stay pending
    // until its own projection is reconciled, not be resolved against this one.
    let session_id = projection.session();
    let hinted: Vec<RunKey> = state
        .pending_hints(store)?
        .into_iter()
        .filter(|(key, hint_cwd)| {
            session_id == Some(key.thread.as_str())
                && project::resolve(Path::new(hint_cwd)).is_ok_and(|h| h.id == p.id)
        })
        .map(|(key, _)| key)
        .collect();
    let turns = projection.completed_turns();
    let mut queued = 0;
    // A hint is authoritative: the engine only captures a run some live process
    // saw end. Each hint resolves to its exact turn; a run that ended before
    // enablement is excluded exactly like a baseline turn.
    for hint in hinted {
        let Ok(turn) = hint.turn.parse::<u64>() else {
            continue;
        };
        let Some(completed) = turns.iter().find(|run| run.turn == turn) else {
            // The bridge writes one projection per turn. A turn absent from this
            // file may still have its own `<session>.<turn>.json` projection, so
            // leave the hint pending for that reconcile instead of excluding it.
            continue;
        };
        if completed.human_messages.is_empty() {
            // The turn is present but has no direct human message. Resolve it as
            // the same terminal exclusion capture uses instead of leaving the
            // hint pending forever.
            state.exclude(&hint, Reason::ExcludedInheritedOrNonhuman.as_str())?;
            state.resolve_hint(&hint)?;
            continue;
        }
        if completed.ended_at <= boundary.millis() {
            state.exclude(&hint, Reason::BeforeEnablement.as_str())?;
            state.resolve_hint(&hint)?;
            continue;
        }
        if state.enqueue_hinted(&hint, &p.id, &cwd, clock::now())? == Enqueue::Inserted {
            queued += 1;
        }
        state.resolve_hint(&hint)?;
    }
    Ok(DshReport::new("reconcile").queued(queued))
}

fn dsh_capture_turn(
    state: &mut State,
    store: &str,
    root: &Path,
    session: &Path,
    project: &Path,
    turn: Option<u64>,
    thread: Option<&str>,
) -> Result<DshReport> {
    let requested = turn.map(|turn| turn.to_string());
    // The bridge names the session, so claim before reading its projection. A
    // missing or malformed document then becomes a held run instead of an error
    // that leaves the already-enqueued run queued forever. Without `--thread`
    // (manual use), the document must name the session first.
    let (claim, document) = if let Some(thread) = thread {
        let Some(claim) = state.claim_session_turn(
            Some(store),
            thread,
            requested.as_deref(),
            clock::now(),
            CAPTURE_LEASE,
        )?
        else {
            return Ok(DshReport::new("capture").reason(Reason::NoReadyRun));
        };
        if !state.hinted(&claim.key)? {
            return hold(state, &claim, "capture", Reason::NoCaptureHint);
        }
        let document = match read_document(session) {
            Ok(document) => document,
            Err(_) => return hold(state, &claim, "capture", Reason::ReaderFailure),
        };
        (claim, document)
    } else {
        let document = read_document(session)?;
        let session_id = document
            .pointer("/header/id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| anyhow::anyhow!("Missing session identity"))?;
        let Some(claim) = state.claim_session_turn(
            Some(store),
            session_id,
            requested.as_deref(),
            clock::now(),
            CAPTURE_LEASE,
        )?
        else {
            return Ok(DshReport::new("capture").reason(Reason::NoReadyRun));
        };
        if !state.hinted(&claim.key)? {
            // Only a hint can name the turn this process saw end. Without one,
            // the projection belongs to a different process or turn and must
            // not be published as this run's evidence.
            return hold(state, &claim, "capture", Reason::NoCaptureHint);
        }
        (claim, document)
    };
    if dsh::projection_schema(&document) != Some(dsh::PROJECTION_SCHEMA) {
        return hold(
            state,
            &claim,
            "capture",
            Reason::UnsupportedProjectionSchema,
        );
    }
    let projection = match dsh::Projection::from_value(document) {
        Ok(projection) => projection,
        // The schema is supported but the document cannot be read: hold the
        // claimed run instead of leasing it.
        Err(_) => return hold(state, &claim, "capture", Reason::ReaderFailure),
    };
    let Some(session) = projection.session() else {
        return hold_raw(state, &claim, "capture", dsh::Reason::Header.reason());
    };
    if session != claim.key.thread {
        // `--thread` located the run, but the document describes another
        // session. Publishing it would attach the wrong evidence.
        return hold(state, &claim, "capture", Reason::ProjectionSessionMismatch);
    }
    let Ok(turn_number) = claim.key.turn.parse::<u64>() else {
        return hold(state, &claim, "capture", Reason::InvalidTurnIdentity);
    };
    // Resolve attribution from the document already read and claimed: re-reading
    // the path could observe a different snapshot (or a file the bridge just
    // deleted) and turn a valid capture into a hold.
    let Ok((target, cwd)) =
        project::resolve(project).and_then(|p| projection_cwd(&p, &projection).map(|cwd| (p, cwd)))
    else {
        // The recorded directory was deleted, moved, or now resolves to another
        // repository: hold it, do not re-queue it.
        return hold(state, &claim, "capture", Reason::ProjectMismatch);
    };
    if claim.project != target.id {
        return hold(state, &claim, "capture", Reason::ProjectMismatch);
    }
    let boundary = project_boundary(state, &claim.project)?;
    let outcome = dsh::decode_after(&projection, store, &cwd, turn_number, boundary)?;
    dsh_capture(state, root, &claim, outcome)
}

/// Read and decode one bounded bridge document.
fn read_document(path: &Path) -> Result<Value> {
    Ok(serde_json::from_slice(&evidence::read_bounded(
        path,
        evidence::MAX_SOURCE_BYTES,
    )?)?)
}

/// The cwd a projection recorded, which must belong to the registered project:
/// a session cannot be attributed to a project it did not run in.
fn projection_cwd(p: &crate::project::Project, projection: &dsh::Projection) -> Result<String> {
    let cwd = projection
        .cwd()
        .ok_or_else(|| anyhow::anyhow!("Projection recorded no cwd"))?;
    recorded_project(p, cwd)
}

/// A recorded cwd must resolve to the registered project.
fn recorded_project(p: &crate::project::Project, cwd: &str) -> Result<String> {
    let recorded = project::resolve(Path::new(cwd))?;
    ensure!(recorded.id == p.id, "project_mismatch");
    Ok(cwd.into())
}

/// The enablement boundary at the finest precision the project record holds.
fn project_boundary(state: &State, id: &str) -> Result<dsh::Boundary> {
    match state.project_epoch_ms(id)? {
        Some(ms) => Ok(dsh::Boundary::Millis(ms)),
        None => Ok(dsh::Boundary::Second(state.project_epoch(id)?)),
    }
}

/// Record a terminal reason and report the held run.
fn hold(
    state: &mut State,
    claim: &Claim,
    action: &'static str,
    reason: Reason,
) -> Result<DshReport> {
    hold_raw(state, claim, action, reason.as_str())
}
fn hold_raw(
    state: &mut State,
    claim: &Claim,
    action: &'static str,
    reason: &'static str,
) -> Result<DshReport> {
    state.hold(claim, reason, clock::now())?;
    Ok(DshReport::new(action)
        .state("held")
        .run(claim.key.clone())
        .reason_str(reason))
}

/// Publish one decoded DSH turn with the same transitions Codex capture uses.
fn dsh_capture(
    state: &mut State,
    root: &Path,
    claim: &Claim,
    outcome: dsh::Capture,
) -> Result<DshReport> {
    use dsh::Capture;
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
            state.finish(claim, &bundle.outcome, Some(&digest), clock::now())?;
            Ok(DshReport::new("capture")
                .state(state_name)
                .run(claim.key.clone())
                .evidence(digest))
        }
        Capture::Pending if claim.attempts >= DSH_PENDING_ATTEMPT_LIMIT => {
            // The same bounded exhaustion the Codex reader applies: a projection
            // that never records the turn's end becomes inspectable instead of
            // retrying forever.
            hold(state, claim, "capture", Reason::HistoryReadinessExhausted)
        }
        Capture::Pending => {
            state.defer(claim, Reason::HistoryPending.as_str(), clock::now(), 10)?;
            Ok(DshReport::new("capture")
                .state("queued")
                .run(claim.key.clone())
                .reason(Reason::HistoryPending))
        }
        Capture::Missing => hold(state, claim, "capture", Reason::HistoryMissing),
        Capture::Unsupported(reason) => hold_raw(state, claim, "capture", reason.reason()),
        Capture::OverLimit => hold(state, claim, "capture", Reason::HistoryOverLimit),
        Capture::BeforeEnablement => hold(state, claim, "capture", Reason::BeforeEnablement),
        Capture::Excluded => hold(state, claim, "capture", Reason::ExcludedInheritedOrNonhuman),
        Capture::WrongProject => hold(state, claim, "capture", Reason::ProjectMismatch),
    }
}

/// Read at most one byte beyond the signal bound, so oversize is detectable.
fn read_signal() -> Result<Vec<u8>> {
    use std::io::Read;
    let mut bytes = Vec::new();
    std::io::stdin()
        .take((dsh::MAX_SIGNAL_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    Ok(bytes)
}
