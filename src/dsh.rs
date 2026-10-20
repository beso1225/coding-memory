//! DeepSeek Harness (DSH) capture adapter: decode a native session log projection.
//!
//! The bridge (`@coding-memory/dsh-capture`) reads one session through DSH's own
//! `sessionQuery` and emits a bounded JSON projection. This module owns every
//! decision the engine makes about that projection: turn boundaries, human-run
//! detection, exclusion, attribution, outcomes, sizes, and evidence shape. It
//! never reads DSH storage directly, so a persisted format change only reaches
//! the bridge.
use crate::{
    domain::{Evidence, RunKey},
    project,
};
use anyhow::{Result, ensure};
use serde::Deserialize;
use serde_json::Value;
use std::{collections::HashSet, path::Path};

/// Bridge document revision this engine interprets. It is separate from the
/// DSH logical session format: the projection's own shape can change alone.
pub const PROJECTION_SCHEMA: u32 = 1;
/// Highest logical session format this adapter interprets. A newer format is
/// held, not guessed: field meanings can change between generations.
pub const MAX_LOG_VERSION: u64 = 4;
/// Bounded projection of one turn, matching the Codex reader's item bound.
pub const MAX_BYTES: usize = 8 * 1024 * 1024;
/// The only message source kind treated as a human run.
const HUMAN: &str = "user";

/// One projected log record. Unused fields are ignored so a newer bridge can add
/// metadata without breaking an older engine.
#[derive(Clone, Debug, Deserialize, serde::Serialize)]
pub struct Record {
    pub seq: Option<u64>,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub time: Option<i64>,
    #[serde(default)]
    pub data: Option<Value>,
}

/// Why a projected run cannot become evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    ProjectionSchema,
    HistoryVersion,
    Header,
    Sequence,
    TurnIdentity,
    TurnAttribution,
    TurnEndReason,
    ItemIdentity,
    DuplicateIdentity,
    StartedAt,
    UnknownEventType,
}
impl Reason {
    pub fn reason(self) -> &'static str {
        match self {
            Self::ProjectionSchema => "unsupported_projection_schema",
            Self::HistoryVersion => "unsupported_history_version",
            Self::Header => "invalid_session_header",
            Self::Sequence => "invalid_session_sequence",
            Self::TurnIdentity => "invalid_turn_identity",
            Self::TurnAttribution => "invalid_turn_attribution",
            Self::TurnEndReason => "unsupported_turn_end_reason",
            Self::ItemIdentity => "invalid_item_identity",
            Self::DuplicateIdentity => "duplicate_item_identity",
            Self::StartedAt => "missing_turn_start",
            Self::UnknownEventType => "unknown_session_event_type",
        }
    }
}

/// The outcome of decoding one turn.
#[derive(Debug)]
pub enum Capture {
    Ready(Box<Evidence>),
    Pending,
    Missing,
    Unsupported(Reason),
    OverLimit,
    Excluded,
    /// The turn ended at or before the project's registration second.
    BeforeEnablement,
    WrongProject,
}

/// The bridge document revision a raw document declares.
///
/// Readable before the rest of the document is trusted, so a caller can claim
/// the run it names and hold it instead of failing before any state change.
pub fn projection_schema(document: &Value) -> Option<u64> {
    document.get("schema").and_then(Value::as_u64)
}

/// A bounded projection of one session, as the bridge writes it.
///
/// Discovery and capture read the same document, so a hint recorded before
/// registration can resolve the exact run it names without a second reader.
#[derive(Debug, Deserialize, serde::Serialize)]
pub struct Projection {
    /// The bridge document revision, independent of DSH's session format.
    schema: u32,
    header: Header,
    records: Vec<Record>,
}
impl Projection {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let projection: Self = serde_json::from_slice(bytes)?;
        ensure!(
            projection.schema == PROJECTION_SCHEMA,
            "Unsupported bridge projection schema"
        );
        Ok(projection)
    }
    /// The working directory this session recorded.
    pub fn cwd(&self) -> Option<&str> {
        self.header.cwd.as_deref()
    }
    /// The session this projection describes.
    pub fn session(&self) -> &str {
        self.header.id.as_deref().unwrap_or_default()
    }
    /// Whether this session continues an earlier one as a resumed process.
    ///
    /// `session/end-seed` is documented as the fork boundary, but a resumed
    /// process appends it too: observed on `agents.resume` with an untouched
    /// header (`isSeeded: false`). It is therefore a resume marker, not proof
    /// of an inherited prefix. The inherited prefix, when one exists, is still
    /// rejected by `inheritedEventCount` (see `inherited_cut`).
    pub fn resumed(&self) -> bool {
        self.records.iter().any(|r| r.kind == "session/end-seed")
    }
    /// Every completed turn of a human run, as discovered by scanning the log.
    pub fn completed_runs(&self) -> Vec<CompletedRun> {
        if exclude_header(&self.header) {
            return Vec::new();
        }
        let (human_seqs, ends) = scan_turns(&self.records);
        let mut human_turns = HashSet::new();
        for record in &self.records {
            if let Some(seq) = record.seq
                && human_seqs.contains(&seq)
                && let Some(turn) = owning_turn(&self.records, seq)
            {
                human_turns.insert(turn);
            }
        }
        ends.into_iter()
            .filter(|(turn, _)| human_turns.contains(turn))
            .map(|(turn, ended_at)| CompletedRun {
                turn,
                ended_at,
                human_messages: self
                    .records
                    .iter()
                    .filter(|r| r.seq.is_some_and(|seq| human_seqs.contains(&seq)))
                    .filter(|r| r.seq.and_then(|seq| owning_turn(&self.records, seq)) == Some(turn))
                    .filter_map(|r| r.seq)
                    .collect(),
            })
            .collect()
    }
}

/// One completed run in a projection.
#[derive(Debug, Clone)]
pub struct CompletedRun {
    pub turn: u64,
    pub ended_at: i64,
    /// Raw seqs of the turn's direct human messages.
    pub human_messages: Vec<u64>,
}

/// One discovered run and its turn's completion time.
#[derive(Debug, Clone)]
pub struct Discovered {
    pub turn: RunKey,
    pub ended_at: i64,
}

/// A projected session: header fields plus contiguous records.
#[derive(Debug, Deserialize)]
struct Log {
    header: Header,
    records: Vec<Record>,
}
#[derive(Debug, Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Header {
    #[serde(rename = "type")]
    kind: Option<String>,
    version: Option<u64>,
    id: Option<String>,
    cwd: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    created_at: Option<i64>,
    #[serde(default)]
    is_seeded: Option<bool>,
    #[serde(default)]
    origin: Option<String>,
    #[serde(default)]
    delegation_depth: Option<u64>,
}

/// Harness bookkeeping the bridge may omit without changing how an owned turn
/// reads. Every other omitted event type is held.
const BOOKKEEPING: &[&str] = &[
    "turn/start",
    // The rendered system prompt is recomposed per step and carries no fact
    // about what the human asked. A completed real turn records it.
    "system/message",
    "turn/end",
    "step/start",
    "step/end",
    "request/header",
    "request/context",
    "agent/inbox/spliced",
    "session/title",
    "session/title-llm-request",
    "permission/preset",
    "sandbox/mode",
    "approval/policy",
    "approval/asked",
    "approval/decided",
    "job/registered",
    "job/settled",
    "session/end-seed",
    // A compaction request is a human command, not the turn's own work. The
    // bracket is appended after the turn, so it cannot be attributed to it; it
    // does not change what the human asked, and a compacted surface is measured
    // in the experiment report rather than inferred here.
    "command/run",
    "command/done",
    "compaction/start",
    "compaction/summary",
    "compaction/prune",
    "compaction/end",
];
/// Event types that become immutable evidence for the owning turn.
const RELEVANT_FOR_EVIDENCE: &[&str] = &[
    "user/message",
    "assistant/message",
    "assistant/attempt",
    "tool/call",
    "tool/result",
    "developer/message",
];

fn parse(log: &Value) -> Result<Log> {
    Ok(serde_json::from_value(log.clone())?)
}
fn text(value: Option<&Value>) -> Option<&str> {
    value?.as_str()
}
fn integer(value: Option<&Value>) -> Option<i64> {
    value?.as_i64()
}

/// Parse a bounded bridge signal into a run key and cwd.
///
/// The signal is a hint: it names a turn and the directory the harness recorded.
/// It authorizes nothing; capture re-reads the session and re-validates.
pub fn signal(value: &Value, store: &str) -> Result<(RunKey, String)> {
    ensure!(
        value["kind"].as_str() == Some("turn-end"),
        "Unsupported DSH capture signal"
    );
    let session = text(value.get("session"))
        .filter(|s| !s.is_empty() && s.len() <= 256)
        .ok_or_else(|| anyhow::anyhow!("Missing session identity"))?;
    let turn = integer(value.get("turn"))
        .filter(|t| *t >= 1)
        .ok_or_else(|| anyhow::anyhow!("Missing turn number"))?;
    ensure!(
        value.get("reason").is_none_or(|r| r.as_str().is_some()),
        "Invalid turn end reason"
    );
    let cwd = text(value.get("cwd"))
        .filter(|s| !s.is_empty() && s.len() <= 4096)
        .ok_or_else(|| anyhow::anyhow!("Missing or invalid cwd"))?;
    Ok((
        RunKey {
            store: store.into(),
            thread: session.into(),
            turn: turn.to_string(),
        },
        cwd.into(),
    ))
}

/// List completed turns whose session recorded `cwd`.
///
/// Discovery is a hint generator, exactly like reconciliation against Codex: a
/// turn here still passes `decode` before evidence exists. Sessions that are
/// subagent children or forks contribute nothing.
pub fn discover(path: &Path, cwd: &str, store: &str) -> Result<Vec<Discovered>> {
    let document: Value = serde_json::from_slice(&crate::evidence::read_bounded(
        path,
        crate::evidence::MAX_SOURCE_BYTES,
    )?)?;
    // Registration fixes the enablement boundary permanently. It must not scan a
    // document whose bridge schema this engine cannot interpret, or the project
    // would be enabled against a baseline it never understood.
    ensure!(
        projection_schema(&document) == Some(u64::from(PROJECTION_SCHEMA)),
        "Unsupported bridge projection schema"
    );
    let log: Log = parse(&document)?;
    // The logical version decides field meaning. Registration must not record a
    // permanent boundary for a generation this engine would later reject on
    // every capture.
    ensure!(
        log.header.kind.as_deref() == Some("session"),
        "Invalid session header"
    );
    ensure!(
        log.header
            .version
            .is_some_and(|version| version <= MAX_LOG_VERSION),
        "Unsupported DSH logical history version"
    );
    if exclude_header(&log.header) || log.header.cwd.as_deref() != Some(cwd) {
        return Ok(Vec::new());
    }
    let id = log
        .header
        .id
        .clone()
        .ok_or_else(|| anyhow::anyhow!("Missing session identity"))?;
    let mut runs = Vec::new();
    let (human_seqs, ends) = scan_turns(&log.records);
    let mut human_turns = HashSet::new();
    for seq in &human_seqs {
        if let Some(turn) = owning_turn(&log.records, *seq) {
            human_turns.insert(turn);
        }
    }
    for (turn, ended_at) in ends {
        if !human_turns.contains(&turn) {
            continue;
        }
        runs.push(Discovered {
            turn: RunKey {
                store: store.into(),
                thread: id.clone(),
                turn: turn.to_string(),
            },
            ended_at,
        });
    }
    Ok(runs)
}

/// Decode one turn of a projected session into evidence.
///
/// `epoch` is the project's registration second. A turn that ended at or before
/// it predates enablement and is excluded, matching Codex's baseline rule.
pub fn decode(log: &Value, store: &str, cwd: &str, turn: u64, epoch: i64) -> Result<Capture> {
    decode_after(log, store, cwd, turn, &Boundary::Second(epoch))
}

/// The instant a project became enabled, at the precision its record allows.
#[derive(Clone, Copy, Debug)]
pub enum Boundary {
    /// Whole-second registration epoch (older records, and Codex's rule).
    Second(i64),
    /// The exact registration instant.
    Millis(i64),
}
impl Boundary {
    /// The boundary as milliseconds, for comparing a turn's millisecond end.
    pub fn millis(self) -> i64 {
        match self {
            Self::Second(epoch) => epoch.saturating_mul(1000),
            Self::Millis(ms) => ms,
        }
    }
}

/// Decode one turn against an enablement boundary of any recorded precision.
pub fn decode_after(
    log: &Value,
    store: &str,
    cwd: &str,
    turn: u64,
    boundary: &Boundary,
) -> Result<Capture> {
    if projection_schema(log) != Some(u64::from(PROJECTION_SCHEMA)) {
        // The bridge's own document shape, not DSH's logical format.
        return Ok(Capture::Unsupported(Reason::ProjectionSchema));
    }
    let log = parse(log)?;
    if log.header.kind.as_deref() != Some("session") || log.header.version.is_none() {
        return Ok(Capture::Unsupported(Reason::Header));
    }
    if log.header.version.is_some_and(|v| v > MAX_LOG_VERSION) {
        return Ok(Capture::Unsupported(Reason::HistoryVersion));
    }
    if exclude_header(&log.header) {
        return Ok(Capture::Excluded);
    }
    let Some(session) = log.header.id.as_deref().filter(|id| !id.is_empty()) else {
        return Ok(Capture::Unsupported(Reason::Header));
    };
    if log.header.cwd.as_deref() != Some(cwd) {
        return Ok(Capture::WrongProject);
    }
    let mut previous: Option<u64> = None;
    for record in &log.records {
        let Some(seq) = record.seq else {
            return Ok(Capture::Unsupported(Reason::Sequence));
        };
        if previous.is_some_and(|p| seq != p + 1) {
            return Ok(Capture::Unsupported(Reason::Sequence));
        }
        previous = Some(seq);
    }
    let Some(start) = log
        .records
        .iter()
        .find(|r| r.kind == "turn/start" && turn_of(r) == Some(turn))
    else {
        return Ok(Capture::Missing);
    };
    let Some(end) = log
        .records
        .iter()
        .find(|r| r.kind == "turn/end" && turn_of(r) == Some(turn))
    else {
        // An open turn may still receive events; a later turn proves this one is
        // no longer open and its end record is genuinely absent.
        let superseded = log
            .records
            .iter()
            .any(|r| r.kind == "turn/start" && turn_of(r).is_some_and(|t| t > turn));
        return Ok(if superseded {
            Capture::Unsupported(Reason::TurnIdentity)
        } else {
            Capture::Pending
        });
    };
    let Some(end_seq) = end.seq else {
        return Ok(Capture::Unsupported(Reason::Sequence));
    };
    let Some(ended_ms) = end.millis() else {
        return Ok(Capture::Unsupported(Reason::TurnIdentity));
    };
    let Some(started_at) = record_time(start) else {
        return Ok(Capture::Unsupported(Reason::StartedAt));
    };
    if ended_ms / 1000 < started_at {
        return Ok(Capture::Unsupported(Reason::StartedAt));
    }
    let reason = match text(
        end.data
            .as_ref()
            .and_then(|d| d.get("reason").and_then(|r| r.get("kind"))),
    ) {
        Some("completed") => "completed",
        Some("error") => "failed",
        Some("aborted") | Some("interrupted") => "interrupted",
        _ => return Ok(Capture::Unsupported(Reason::TurnEndReason)),
    };
    // A turn ending in the same second but after registration is post-enablement.
    if ended_ms <= boundary.millis() {
        return Ok(Capture::BeforeEnablement);
    }
    let ended_at = ended_ms / 1000;
    let mut ids = HashSet::new();
    // Call identities seen in this turn, and the subset already answered.
    let mut calls = HashSet::new();
    let mut results = HashSet::new();
    let mut items = Vec::new();
    // The bound covers every projected record of the turn, including attempts
    // that never become items: the projection's own size is the cost paid.
    let mut bytes = 0;
    let mut human = false;
    for record in &log.records {
        let Some(seq) = record.seq else {
            return Ok(Capture::Unsupported(Reason::Sequence));
        };
        if seq <= start_seq_of(&log, turn) || seq >= end_seq {
            continue;
        }
        let Some(data) = record.data.as_ref() else {
            return Ok(Capture::Unsupported(Reason::TurnIdentity));
        };
        // Only records that carry a turn field can disagree with the turned-end
        // turn. A message's ownership comes from the enclosing `turn/start`,
        // because DSH records no `turn` on `user/message`.
        if let Some(attributed) = integer(data.get("turn")) {
            if attributed != turn as i64 {
                return Ok(Capture::Unsupported(Reason::TurnAttribution));
            }
        } else if owning_turn(&log.records, seq) != Some(turn) {
            return Ok(Capture::Unsupported(Reason::TurnAttribution));
        }
        let human_message = record.kind == "user/message"
            && text(data.get("source").and_then(|s| s.get("kind"))) == Some(HUMAN);
        if human_message {
            human = true;
        } else if record.kind == "user/message" {
            // Injected context is the harness's framing, not the human's run. It
            // is projected away instead of becoming reusable-looking evidence.
            continue;
        }
        if BOOKKEEPING.contains(&record.kind.as_str()) {
            continue;
        }
        if !RELEVANT_FOR_EVIDENCE.contains(&record.kind.as_str()) {
            // An unprojected event is safe only when its omission cannot change
            // how this turn reads. Unknown vocabulary, or an event the bridge
            // deliberately dropped, is held instead of silently narrowed.
            return Ok(Capture::Unsupported(Reason::UnknownEventType));
        }
        if record.kind == "assistant/attempt" {
            // A settled attempt keeps its exact compact stream, which is never
            // rendered. The committed message that carries it is evidence, so an
            // attempt only contributes to the size bound.
            bytes += serde_json::to_vec(data).map_or(usize::MAX, |v| v.len());
            if bytes > MAX_BYTES {
                return Ok(Capture::OverLimit);
            }
            continue;
        }
        let Some(identity) = projected_identity(record) else {
            return Ok(Capture::Unsupported(Reason::ItemIdentity));
        };
        if record.kind == "tool/result" {
            // A result is identified by the call it answers, which must already
            // be in this turn: an orphan or a repeat would otherwise become a
            // "completed" commandExecution and pass as observed support.
            if identity.is_empty()
                || !calls.contains(identity)
                || !results.insert(identity.to_owned())
            {
                return Ok(Capture::Unsupported(Reason::ItemIdentity));
            }
        } else {
            if !assert_identity(identity, &mut ids) {
                return Ok(Capture::Unsupported(Reason::DuplicateIdentity));
            }
            if record.kind == "tool/call" {
                calls.insert(identity.to_owned());
            }
        }
        // Normalized into the shared evidence item envelope: extraction builds
        // its citation schema from a top-level string `id` and verifies support
        // against the Codex vocabulary (`userMessage`, completed tool items).
        // A raw DSH record has no top-level id, so an unnormalized bundle would
        // admit only an empty candidate list.
        let item = serde_json::json!({
            "id": item_id(identity, seq),
            "seq": seq,
            "type": item_type(record.kind.as_str()),
            "ds_type": record.kind,
            "status": item_status(record.kind.as_str(), data),
            "data": data,
        });
        bytes += serde_json::to_vec(&item).map_or(usize::MAX, |v| v.len());
        items.push(item);
    }
    if !human {
        return Ok(Capture::Excluded);
    }
    if bytes > MAX_BYTES {
        return Ok(Capture::OverLimit);
    }
    Ok(Capture::Ready(Box::new(Evidence {
        schema_version: 1,
        run: RunKey {
            store: store.into(),
            thread: session.into(),
            turn: turn.to_string(),
        },
        project: String::new(),
        runtime_version: log.header.version.unwrap_or_default().to_string(),
        capture_runtime: None,
        outcome: reason.into(),
        completed_at: ended_at,
        started_at,
        cwd: Some(cwd.into()),
        terminal_error: end
            .data
            .as_ref()
            .and_then(|d| d.get("reason"))
            .and_then(|r| r.get("error"))
            .cloned(),
        items,
        coverage: vec![
            format!(
                "DSH sessionQuery projection of a dsh-session v{} logical log",
                log.header.version.unwrap_or_default()
            ),
            "Injected context, job, and approval events are projected away".into(),
            "Attachment bytes, provider-hidden reasoning, and replaced surface nodes are not captured"
                .into(),
        ],
    })))
}

/// The identity a projected record contributes to evidence.
///
/// A user message is a message; a developer message identifies itself; a tool
/// result is identified by the call it answers, which also survives an outer
/// `id` added by projection metadata. Preferring an outer `id` would let one
/// turn's envelope collide with another record's native identity.
/// The evidence item id: the native projection identity, kept unique per turn
/// because a turn is the whole evidence scope.
fn item_id(identity: &str, seq: u64) -> String {
    format!("{identity}@{seq}")
}
/// The shared evidence vocabulary an extraction run understands.
///
/// DSH's own names never matched the predicates extraction checks, so a tool
/// result could not stand as observed support even if it had been citable.
fn item_type(kind: &str) -> &'static str {
    match kind {
        "user/message" => "userMessage",
        "assistant/message" => "agentMessage",
        "developer/message" => "hookPrompt",
        "tool/call" => "toolCall",
        "tool/result" => "commandExecution",
        _ => "other",
    }
}
/// A tool result's status, read from the recorded message, or `None`.
fn item_status(kind: &str, data: &Value) -> Option<&'static str> {
    if kind != "tool/result" {
        return None;
    }
    Some(
        match data
            .get("message")
            .and_then(|m| m.get("isError"))
            .and_then(Value::as_bool)
        {
            Some(true) => "failed",
            _ => "completed",
        },
    )
}
fn projected_identity(record: &Record) -> Option<&str> {
    let data = record.data.as_ref()?;
    text(match record.kind.as_str() {
        "user/message" | "developer/message" => data.get("id"),
        "assistant/message" => data.get("message").and_then(|m| m.get("id")),
        "assistant/attempt" => data.get("attemptId").or_else(|| data.get("id")),
        "tool/result" => data
            .get("message")
            .and_then(|m| m.get("toolCallId"))
            .or_else(|| data.get("toolCallId"))
            .or_else(|| data.get("id")),
        _ => data.get("callId"),
    })
}
/// A missing or colliding projection identity makes the turn inspectable rather
/// than silently merging two items.
fn assert_identity(identity: &str, ids: &mut HashSet<String>) -> bool {
    !identity.is_empty() && ids.insert(identity.to_owned())
}

/// The direct human prompts and completed turns in a raw log.
///
/// A `user/message` carries **no** `turn` field in DSH; it belongs to the turn
/// opened by the nearest preceding `turn/start`. Scanning by attribute instead
/// of by ownership leaves every real turn looking non-human.
fn scan_turns(records: &[Record]) -> (HashSet<u64>, Vec<(u64, i64)>) {
    // Ends are in milliseconds: an enablement boundary is second-resolution,
    // and truncating first would exclude a turn that ended later in the same
    // second as registration.
    let mut humans = HashSet::new();
    let mut ends = Vec::new();
    for record in records {
        if is_human_message(record)
            && let Some(seq) = record.seq
        {
            humans.insert(seq);
        }
        if record.kind == "turn/end"
            && let Some(turn) = turn_of(record)
            && let Some(ended) = record.millis()
        {
            ends.push((turn, ended));
        }
    }
    (humans, ends)
}

/// The turn that owns `seq`: the nearest `turn/start` at or before it.
fn owning_turn(records: &[Record], seq: u64) -> Option<u64> {
    records
        .iter()
        .filter(|r| r.kind == "turn/start" && r.seq.is_some_and(|s| s <= seq))
        .max_by_key(|r| r.seq)
        .and_then(turn_of)
}

/// Whether a record is a direct human prompt rather than injected context.
fn is_human_message(record: &Record) -> bool {
    record.kind == "user/message"
        && record
            .data
            .as_ref()
            .and_then(|d| d.get("source"))
            .and_then(|s| s.get("kind"))
            .and_then(Value::as_str)
            == Some(HUMAN)
}
fn exclude_header(header: &Header) -> bool {
    header.is_seeded == Some(true)
        || header.origin.as_deref() == Some("subagent")
        || header.delegation_depth.is_some_and(|d| d > 0)
}
fn turn_of(record: &Record) -> Option<u64> {
    record
        .data
        .as_ref()
        .and_then(|d| d.get("turn"))
        .and_then(Value::as_u64)
}
/// A record's committed time in Unix **seconds**, the unit the engine's
/// enablement epoch uses. DSH commits milliseconds; comparing them directly
/// would make every enablement check false and admit pre-registration turns.
fn record_time(record: &Record) -> Option<i64> {
    record.time.map(|millis| millis / 1000)
}
impl Record {
    /// The committed time in milliseconds, as DSH records it.
    fn millis(&self) -> Option<i64> {
        self.time
    }
}
fn start_seq_of(log: &Log, turn: u64) -> u64 {
    log.records
        .iter()
        .find(|r| r.kind == "turn/start" && turn_of(r) == Some(turn))
        .and_then(|r| r.seq)
        .unwrap_or(0)
}

/// Resolve the project directory a bridge invocation named.
pub fn project_at(cwd: &str) -> Result<project::Project> {
    project::resolve(Path::new(cwd))
}
