//! DeepSeek Harness (DSH) capture adapter: decode a native session log projection.
//!
//! The bridge (`@coding-memory/dsh-capture`) reads one session through DSH's own
//! `sessionQuery` and emits a bounded JSON projection. This module owns every
//! decision the engine makes about that projection: turn boundaries, human-run
//! detection, exclusion, attribution, outcomes, sizes, and evidence shape. It
//! never reads DSH storage directly, so a persisted format change only reaches
//! the bridge.
use crate::domain::{Evidence, RunKey};
use anyhow::{Result, ensure};
use serde::Deserialize;
use serde_json::Value;
use std::collections::{HashMap, HashSet};

/// Bridge document revision this engine interprets. It is separate from the
/// DSH logical session format: the projection's own shape can change alone.
pub const PROJECTION_SCHEMA: u64 = 1;
/// Highest logical session format this adapter interprets. A newer format is
/// held, not guessed: field meanings can change between generations.
pub const MAX_LOG_VERSION: u64 = 4;
/// Bounded projection of one turn, matching the Codex reader's item bound.
pub const MAX_BYTES: usize = 8 * 1024 * 1024;
/// Largest completion hint the bridge may send to `dsh enqueue`.
pub const MAX_SIGNAL_BYTES: usize = 64 * 1024;
/// Identity bounds shared with the bridge's config and projection writer.
const MAX_SESSION_ID: usize = 256;
const MAX_CWD: usize = 4096;
/// The only message source kind treated as a human run.
const HUMAN: &str = "user";

/// One projected log record. Unused fields are ignored so a newer bridge can add
/// metadata without breaking an older engine.
#[derive(Clone, Debug, Deserialize)]
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
    HistoryVersion,
    Header,
    Sequence,
    TurnIdentity,
    TurnAttribution,
    TurnEndReason,
    ItemIdentity,
    DuplicateIdentity,
    StartedAt,
    TurnInterval,
    UnknownEventType,
}
impl Reason {
    pub fn reason(self) -> &'static str {
        match self {
            Self::HistoryVersion => "unsupported_history_version",
            Self::Header => "invalid_session_header",
            Self::Sequence => "invalid_session_sequence",
            Self::TurnIdentity => "invalid_turn_identity",
            Self::TurnAttribution => "invalid_turn_attribution",
            Self::TurnEndReason => "unsupported_turn_end_reason",
            Self::ItemIdentity => "invalid_item_identity",
            Self::DuplicateIdentity => "duplicate_item_identity",
            Self::StartedAt => "missing_turn_start",
            Self::TurnInterval => "invalid_turn_interval",
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
    /// The turn ended at or before the project's registration instant.
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
#[derive(Debug, Deserialize)]
pub struct Projection {
    // The bridge revision is checked on the raw document before this shape is
    // decoded, so it is deliberately not retained here.
    header: Header,
    records: Vec<Record>,
}
impl Projection {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        Self::from_value(serde_json::from_slice(bytes)?)
    }
    /// Parse an already decoded document, checking the bridge revision here so
    /// every caller of the typed projection has the same guarantee.
    ///
    /// The revision is read from the raw document before its shape is decoded:
    /// a future schema may change fields, and reporting it as a deserialization
    /// failure would hide the one fact the engine can act on.
    pub fn from_value(document: Value) -> Result<Self> {
        ensure!(
            projection_schema(&document) == Some(PROJECTION_SCHEMA),
            "Unsupported bridge projection schema"
        );
        Ok(serde_json::from_value(document)?)
    }
    /// The working directory this session recorded.
    pub fn cwd(&self) -> Option<&str> {
        self.header.cwd.as_deref()
    }
    /// The session this projection describes.
    pub fn session(&self) -> Option<&str> {
        self.header.id.as_deref().filter(|id| !id.is_empty())
    }
    /// Whether this session continues an earlier one as a resumed process.
    ///
    /// `session/end-seed` is documented as the fork boundary, but a resumed
    /// process appends it too: observed on `agents.resume` with an untouched
    /// header (`isSeeded: false`). It is therefore a resume marker, not proof
    /// of an inherited prefix. A fork carries `isSeeded` or `parentSession`, and
    /// the whole session is excluded rather than partially captured.
    pub fn resumed(&self) -> bool {
        self.records.iter().any(|r| r.kind == "session/end-seed")
    }
    /// Every completed turn of a human run, as discovered by scanning the log.
    pub fn completed_runs(&self) -> Vec<CompletedRun> {
        self.completed_turns()
            .into_iter()
            .filter(|run| !run.human_messages.is_empty())
            .collect()
    }
    /// Every completed turn, including turns with no direct human message.
    ///
    /// Reconciliation needs the difference between "this turn is absent from
    /// the projection" (keep the hint pending) and "this turn is present but
    /// nonhuman" (resolve the hint as excluded).
    pub fn completed_turns(&self) -> Vec<CompletedRun> {
        if self.excluded() {
            return Vec::new();
        }
        let mut scan = scan_turns(&self.records);
        scan.ends
            .into_iter()
            .map(|(turn, ended_at)| CompletedRun {
                turn,
                ended_at,
                human_messages: scan.humans.remove(&turn).unwrap_or_default(),
            })
            .collect()
    }
    fn excluded(&self) -> bool {
        exclude_header(&self.header)
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

/// A projected session header.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Header {
    #[serde(rename = "type")]
    kind: Option<String>,
    version: Option<u64>,
    id: Option<String>,
    cwd: Option<String>,
    #[serde(default)]
    #[serde(rename = "createdAt")]
    _created_at: Option<i64>,
    #[serde(default)]
    is_seeded: Option<bool>,
    #[serde(default)]
    origin: Option<String>,
    #[serde(default)]
    delegation_depth: Option<u64>,
    /// Lineage of a fork or subagent. The bridge emits it whenever DSH records
    /// one, so the header must accept it even though exclusion does not depend
    /// on the value.
    #[serde(default)]
    parent_session: Option<String>,
}
impl Header {
    /// The logical format version, or why the header cannot be interpreted.
    fn validate(&self) -> Result<u64, Reason> {
        if self.kind.as_deref() != Some("session") {
            return Err(Reason::Header);
        }
        let Some(version) = self.version else {
            return Err(Reason::Header);
        };
        if version > MAX_LOG_VERSION {
            return Err(Reason::HistoryVersion);
        }
        Ok(version)
    }
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
    // The workspace-changes announcement names only the turn it belongs to.
    // The changed-file summary stays on the Host, is keyed by the event's seq,
    // and is never model-visible, so the log record itself carries no fact
    // about the ask.
    "workspace/changes",
    // A provider retry pair is transient transport bookkeeping inside one turn
    // and step. The attempt that settled the step is already
    // `assistant/message`, and a terminal failure is recorded on `turn/end`.
    "llm/retry",
    "llm/retry-started",
    // The todo list is a model-written, whole-list last-write-wins snapshot
    // with no native identity. It is the model's plan, not the human's ask,
    // and capturing it would add items the extraction vocabulary cannot cite
    // or tell apart across snapshots.
    "todo/write",
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

/// List completed human turns whose session recorded `cwd`.
///
/// Discovery is a hint generator, exactly like reconciliation against Codex: a
/// turn here still passes `decode` before evidence exists. Sessions that are
/// subagent children or forks contribute nothing.
pub fn discover(projection: &Projection, cwd: &str, store: &str) -> Result<Vec<Discovered>> {
    // Registration fixes the enablement boundary permanently. It must not scan a
    // document whose logical format this engine cannot interpret, or the project
    // would be enabled against a baseline it never understood.
    projection
        .header
        .validate()
        .map_err(|reason| anyhow::anyhow!("{}", reason.reason()))?;
    if projection.excluded() || projection.cwd() != Some(cwd) {
        return Ok(Vec::new());
    }
    let session = projection
        .session()
        .ok_or_else(|| anyhow::anyhow!("Missing session identity"))?;
    Ok(projection
        .completed_runs()
        .into_iter()
        .map(|run| Discovered {
            turn: RunKey {
                store: store.into(),
                thread: session.into(),
                turn: run.turn.to_string(),
            },
            ended_at: run.ended_at,
        })
        .collect())
}

/// Decode one turn of a projected session into evidence.
///
/// `epoch` is the project's registration second. A turn that ended at or before
/// it predates enablement and is excluded, matching Codex's baseline rule.
pub fn decode(
    projection: &Projection,
    store: &str,
    cwd: &str,
    turn: u64,
    epoch: i64,
) -> Result<Capture> {
    decode_after(projection, store, cwd, turn, Boundary::Second(epoch))
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
    projection: &Projection,
    store: &str,
    cwd: &str,
    turn: u64,
    boundary: Boundary,
) -> Result<Capture> {
    let version = match projection.header.validate() {
        Ok(version) => version,
        Err(reason) => return Ok(Capture::Unsupported(reason)),
    };
    if projection.excluded() {
        return Ok(Capture::Excluded);
    }
    let Some(session) = projection.session() else {
        return Ok(Capture::Unsupported(Reason::Header));
    };
    if projection.cwd() != Some(cwd) {
        return Ok(Capture::WrongProject);
    }
    if !contiguous(&projection.records) {
        return Ok(Capture::Unsupported(Reason::Sequence));
    }
    let Some(start) = projection
        .records
        .iter()
        .find(|r| r.kind == "turn/start" && turn_of(r) == Some(turn))
    else {
        return Ok(Capture::Missing);
    };
    let Some(end) = projection
        .records
        .iter()
        .find(|r| r.kind == "turn/end" && turn_of(r) == Some(turn))
    else {
        // An open turn may still receive events; a later turn proves this one is
        // no longer open and its end record is genuinely absent.
        let superseded = projection
            .records
            .iter()
            .any(|r| r.kind == "turn/start" && turn_of(r).is_some_and(|t| t > turn));
        return Ok(if superseded {
            Capture::Unsupported(Reason::TurnIdentity)
        } else {
            Capture::Pending
        });
    };
    let (Some(start_seq), Some(end_seq)) = (start.seq, end.seq) else {
        return Ok(Capture::Unsupported(Reason::Sequence));
    };
    let Some(ended_ms) = end.time else {
        return Ok(Capture::Unsupported(Reason::TurnIdentity));
    };
    let Some(started_ms) = start.time else {
        return Ok(Capture::Unsupported(Reason::StartedAt));
    };
    // Compare the recorded instants, not their truncated seconds: an end before
    // its start inside one second would otherwise look ordered.
    if ended_ms < started_ms {
        return Ok(Capture::Unsupported(Reason::TurnInterval));
    }
    let started_at = started_ms / 1000;
    let Some(reason) = end_reason(end) else {
        return Ok(Capture::Unsupported(Reason::TurnEndReason));
    };
    // A turn ending in the same second but after registration is post-enablement.
    if ended_ms <= boundary.millis() {
        return Ok(Capture::BeforeEnablement);
    }
    let ended_at = ended_ms / 1000;
    let turn_number = i64::try_from(turn).unwrap_or(i64::MAX);
    let mut ids = HashSet::new();
    // Call identities seen in this turn, and the subset already answered.
    let mut calls = HashSet::new();
    let mut results = HashSet::new();
    let mut items = Vec::new();
    // The bound covers every projected record of the turn, including attempts
    // that never become items: the projection's own size is the cost paid.
    let mut bytes: usize = 0;
    let mut human = false;
    // The turn opened by the nearest preceding `turn/start`. DSH records no
    // `turn` on `user/message`, so ownership comes from this walk rather than
    // from each record's own payload.
    let mut owner = None;
    for record in &projection.records {
        if record.kind == "turn/start" {
            let started = turn_of(record);
            // A start for another turn inside this turn's window breaks the
            // ownership the rest of the walk relies on. It is not bookkeeping.
            if record
                .seq
                .is_some_and(|seq| seq > start_seq && seq < end_seq)
                && started != Some(turn)
            {
                return Ok(Capture::Unsupported(Reason::TurnAttribution));
            }
            owner = started;
            continue;
        }
        let Some(seq) = record.seq else {
            return Ok(Capture::Unsupported(Reason::Sequence));
        };
        if seq <= start_seq || seq >= end_seq {
            continue;
        }
        // Attribution is checked whenever a payload exists, including for
        // bookkeeping: a record that names another turn means the window is
        // misattributed, even if the record itself would be skipped.
        let data = record.data.as_ref();
        if let Some(data) = data {
            // Only records that carry a turn field can disagree with the
            // turn-end turn. A message's ownership comes from the enclosing
            // `turn/start`.
            if let Some(attributed) = data.get("turn").and_then(Value::as_i64) {
                if attributed != turn_number {
                    return Ok(Capture::Unsupported(Reason::TurnAttribution));
                }
            } else if owner != Some(turn) {
                return Ok(Capture::Unsupported(Reason::TurnAttribution));
            }
        }
        // Bookkeeping may be omitted entirely, so requiring a payload from it
        // would hold a turn the bridge could have safely projected less of.
        if BOOKKEEPING.contains(&record.kind.as_str()) {
            continue;
        }
        if !RELEVANT_FOR_EVIDENCE.contains(&record.kind.as_str()) {
            // An unprojected event is safe only when its omission cannot change
            // how this turn reads. Unknown vocabulary, or an event the bridge
            // deliberately dropped, is held instead of silently narrowed.
            return Ok(Capture::Unsupported(Reason::UnknownEventType));
        }
        let Some(data) = data else {
            return Ok(Capture::Unsupported(Reason::TurnIdentity));
        };
        let human_message = record.kind == "user/message" && is_human_message(record);
        if human_message {
            human = true;
        } else if record.kind == "user/message" {
            // Injected context is the harness's framing, not the human's run. It
            // is projected away instead of becoming reusable-looking evidence.
            continue;
        }
        if record.kind == "assistant/attempt" {
            // A settled attempt keeps its exact compact stream, which is never
            // rendered. The committed message that carries it is evidence, so an
            // attempt only contributes to the size bound.
            bytes = bytes.saturating_add(serde_json::to_vec(data).map_or(usize::MAX, |v| v.len()));
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
            "data": evidence_data(record.kind.as_str(), data),
        });
        bytes = bytes.saturating_add(serde_json::to_vec(&item).map_or(usize::MAX, |v| v.len()));
        items.push(item);
        if bytes > MAX_BYTES {
            return Ok(Capture::OverLimit);
        }
    }
    if !human {
        return Ok(Capture::Excluded);
    }
    Ok(Capture::Ready(Box::new(Evidence {
        schema_version: 1,
        run: RunKey {
            store: store.into(),
            thread: session.into(),
            turn: turn.to_string(),
        },
        project: String::new(),
        // DSH exposes its logical log format, not an application version. The
        // prefix keeps this value distinguishable from a Codex runtime version.
        runtime_version: format!("dsh-log-v{version}"),
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
                "DSH sessionQuery projection of a dsh-session v{version} logical log"
            ),
            "Injected context, job, and approval events are projected away".into(),
            "Attachment bytes, provider-hidden reasoning streams, reasoning blocks, and replaced surface nodes are not captured"
                .into(),
        ],
    })))
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
    let session = value
        .get("session")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty() && s.len() <= MAX_SESSION_ID)
        .ok_or_else(|| anyhow::anyhow!("Missing session identity"))?;
    let turn = value
        .get("turn")
        .and_then(Value::as_i64)
        .filter(|t| *t >= 1)
        .ok_or_else(|| anyhow::anyhow!("Missing turn number"))?;
    ensure!(
        value.get("reason").is_none_or(|r| r.as_str().is_some()),
        "Invalid turn end reason"
    );
    let cwd = value
        .get("cwd")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty() && s.len() <= MAX_CWD)
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

/// Whether every projected record carries a seq and the values are contiguous.
///
/// A bounded projection starts at the captured turn's native seq, so only the
/// step is required: the engine must not assume seq starts at zero.
fn contiguous(records: &[Record]) -> bool {
    let mut previous: Option<u64> = None;
    for record in records {
        let Some(seq) = record.seq else {
            return false;
        };
        if previous.is_some_and(|p| seq != p + 1) {
            return false;
        }
        previous = Some(seq);
    }
    true
}

/// The end reason, mapped onto the closed outcome set evidence understands.
fn end_reason(end: &Record) -> Option<&'static str> {
    match end.data.as_ref()?.get("reason")?.get("kind")?.as_str()? {
        "completed" => Some("completed"),
        "error" => Some("failed"),
        "aborted" | "interrupted" => Some("interrupted"),
        _ => None,
    }
}

/// The direct human prompts and completed turns in a raw log.
///
/// A `user/message` carries **no** `turn` field in DSH; it belongs to the turn
/// opened by the nearest preceding `turn/start`. Scanning by attribute instead
/// of by ownership leaves every real turn looking non-human.
struct TurnScan {
    humans: HashMap<u64, Vec<u64>>,
    ends: Vec<(u64, i64)>,
}

fn scan_turns(records: &[Record]) -> TurnScan {
    // Ends are in milliseconds: an enablement boundary is second-resolution,
    // and truncating first would exclude a turn that ended later in the same
    // second as registration.
    let mut scan = TurnScan {
        humans: HashMap::new(),
        ends: Vec::new(),
    };
    let mut owner = None;
    for record in records {
        if record.kind == "turn/start" {
            owner = turn_of(record);
            continue;
        }
        if record.kind == "turn/end" {
            if let (Some(turn), Some(ended)) = (turn_of(record), record.time) {
                scan.ends.push((turn, ended));
            }
            continue;
        }
        if is_human_message(record)
            && let (Some(turn), Some(seq)) = (owner, record.seq)
        {
            scan.humans.entry(turn).or_default().push(seq);
        }
    }
    scan
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
        || header.parent_session.is_some()
}

fn turn_of(record: &Record) -> Option<u64> {
    record
        .data
        .as_ref()
        .and_then(|d| d.get("turn"))
        .and_then(Value::as_u64)
}

/// The identity a projected record contributes to evidence.
///
/// A user message is a message; a developer message identifies itself; a tool
/// result is identified by the call it answers, which also survives an outer
/// `id` added by projection metadata. Preferring an outer `id` would let one
/// turn's envelope collide with another record's native identity.
fn projected_identity(record: &Record) -> Option<&str> {
    let data = record.data.as_ref()?;
    let field = match record.kind.as_str() {
        "user/message" => data.get("id"),
        // A developer message nests its identity like every other message; an
        // outer `id` is accepted only as a fallback for older projections.
        "developer/message" => data
            .get("message")
            .and_then(|m| m.get("id"))
            .or_else(|| data.get("id")),
        "assistant/message" => data.get("message").and_then(|m| m.get("id")),
        "tool/result" => data
            .get("message")
            .and_then(|m| m.get("toolCallId"))
            .or_else(|| data.get("toolCallId"))
            .or_else(|| data.get("id")),
        _ => data.get("callId"),
    };
    field.and_then(Value::as_str)
}
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
    // A result without its recorded message is not observed support; identity
    // may still come from a top-level `toolCallId`.
    let message = data.get("message")?;
    Some(match message.get("isError").and_then(Value::as_bool) {
        Some(true) => "failed",
        _ => "completed",
    })
}
/// The payload an evidence item keeps for one projected record.
///
/// An `assistant/message` carries the exact timed model stream, including
/// reasoning chunks. The rendered message is the evidence; storing the stream
/// would put hidden reasoning into extraction prompts and spend the size bound
/// on bytes the coverage statement says are not captured. Reasoning content
/// blocks are dropped for the same reason.
fn evidence_data(kind: &str, data: &Value) -> Value {
    let mut value = data.clone();
    if kind == "assistant/message"
        && let Some(object) = value.as_object_mut()
    {
        object.remove("stream");
    }
    let content = if kind == "user/message" {
        value.get_mut("content")
    } else {
        value
            .get_mut("message")
            .and_then(|message| message.get_mut("content"))
    };
    if let Some(Value::Array(blocks)) = content {
        blocks.retain(|block| block.get("type").and_then(Value::as_str) != Some("reasoning"));
    }
    value
}

/// A missing or colliding projection identity makes the turn inspectable rather
/// than silently merging two items.
fn assert_identity(identity: &str, ids: &mut HashSet<String>) -> bool {
    !identity.is_empty() && ids.insert(identity.to_owned())
}
