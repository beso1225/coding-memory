use coding_memory::dsh::{self, Capture, Reason};
use serde_json::{Value, json};
use tempfile::tempdir;

fn header(id: &str, cwd: &str) -> serde_json::Value {
    json!({
        "type": "session",
        "version": 4,
        "id": id,
        "createdAt": 1000,
        "cwd": cwd,
        "isSeeded": false
    })
}
/// One projected record, shaped exactly as the bridge emits it.
fn record(seq: u64, kind: &str, data: Value) -> Value {
    // DSH commits milliseconds.
    let time = (1000 + seq as i64) * 1000;
    json!({"seq": seq, "type": kind, "time": time, "data": data})
}
fn human(seq: u64, turn: u64, step: u64) -> Value {
    record(
        seq,
        "user/message",
        json!({
            "turn": turn, "step": step,
            "id": format!("msg-{seq}"),
            "role": "user",
            "content": [{"type": "text", "text": "please inspect the worker"}],
            "source": {"kind": "user"}
        }),
    )
}
fn injected(seq: u64, turn: u64, step: u64, kind: &str) -> Value {
    record(
        seq,
        "user/message",
        json!({
            "turn": turn, "step": step,
            "id": format!("msg-{seq}"),
            "role": "user",
            "content": [{"type": "text", "text": "injected context"}],
            "source": {"kind": kind}
        }),
    )
}
fn turn_start(seq: u64, turn: u64) -> Value {
    record(seq, "turn/start", json!({"turn": turn}))
}
fn turn_end(seq: u64, turn: u64, reason: Value) -> Value {
    record(seq, "turn/end", json!({"turn": turn, "reason": reason}))
}
fn log(turns: Vec<Value>) -> Value {
    let mut records = vec![
        record(0, "permission/preset", json!({"preset": "default"})),
        record(1, "sandbox/mode", json!({"mode": "workspace"})),
        record(2, "approval/policy", json!({"policy": "ask"})),
    ];
    records.extend(turns);
    json!({"schema": 1, "header": header("session-a", "/repo"), "records": records})
}

fn decode(log: &serde_json::Value, turn: u64) -> Capture {
    dsh::decode(log, "storedigest", "/repo", turn, 3).expect("decode")
}

fn decode_at(log: &serde_json::Value, turn: u64, epoch: i64) -> Capture {
    dsh::decode(log, "storedigest", "/repo", turn, epoch).expect("decode")
}

fn two_turn_log() -> serde_json::Value {
    log(vec![
        turn_start(3, 1),
        injected(4, 1, 1, "time-context"),
        human(5, 1, 1),
        record(
            6,
            "tool/call",
            json!({"turn":1,"step":1,"callId":"c1","name":"read","arguments":{"path":"a"}}),
        ),
        record(
            7,
            "tool/result",
            json!({"turn":1,"step":1,"message":{"id":"t1","role":"tool","toolCallId":"c1","content":[{"type":"text","text":"file"}]},"sourceEventSeqs":[6]}),
        ),
        turn_end(8, 1, json!({"kind": "completed"})),
        turn_start(9, 2),
        human(10, 2, 1),
        turn_end(11, 2, json!({"kind": "completed"})),
    ])
}

#[test]
fn completed_turn_becomes_evidence_scoped_to_its_turn() {
    let Capture::Ready(bundle) = decode(&two_turn_log(), 1) else {
        panic!("expected a ready capture");
    };
    assert_eq!(bundle.outcome, "completed");
    assert_eq!(bundle.runtime_version, "4");
    assert_eq!(bundle.cwd.as_deref(), Some("/repo"));
    let types: Vec<_> = bundle
        .items
        .iter()
        .map(|i| i["type"].as_str().unwrap())
        .collect();
    assert_eq!(
        types,
        vec!["userMessage", "toolCall", "commandExecution"],
        "items use the shared evidence vocabulary extraction understands"
    );
    assert_eq!(
        bundle.items[0]["id"],
        json!("msg-5@5"),
        "top-level ids are citable"
    );
    assert_eq!(
        bundle.items[2]["status"],
        json!("completed"),
        "tool results carry a status"
    );
    assert_eq!(
        bundle.items[2]["ds_type"],
        json!("tool/result"),
        "the native type is retained"
    );
    let ids: Vec<_> = bundle
        .items
        .iter()
        .map(|i| i["seq"].as_u64().unwrap())
        .collect();
    assert_eq!(
        ids,
        vec![5, 6, 7],
        "injected context and bookkeeping excluded"
    );
    assert!(
        ids.iter().all(|seq| *seq < 9),
        "later turns never enter evidence"
    );
    assert!(
        bundle
            .coverage
            .iter()
            .any(|c| c.contains("dsh-session") && c.contains("v4")),
        "coverage names the source log"
    );
    assert_eq!(
        bundle.started_at, 1003,
        "millisecond record times become seconds"
    );
    assert_eq!(bundle.completed_at, 1008);
    assert!(bundle.run.thread == "session-a" && bundle.run.turn == "1");
}

#[test]
fn a_settled_attempt_does_not_become_an_item() {
    // A real completed turn records `assistant/attempt` (with its compact stream)
    // before the committed `assistant/message`. The message is evidence; the
    // attempt only counts toward the size bound.
    let value = log(vec![
        turn_start(3, 1),
        human(4, 1, 1),
        record(
            5,
            "assistant/attempt",
            json!({"turn":1,"step":1,"attemptId":"attempt-1","stream":[{"type":"text","text":"ok"}]}),
        ),
        record(
            6,
            "assistant/message",
            json!({"turn":1,"step":1,"message":{"id":"assistant-1","role":"assistant","content":[{"type":"text","text":"ok"}],"source":{"kind":"model"}}}),
        ),
        turn_end(7, 1, json!({"kind": "completed"})),
    ]);
    let Capture::Ready(bundle) = decode(&value, 1) else {
        panic!("expected a ready capture");
    };
    let types: Vec<_> = bundle
        .items
        .iter()
        .map(|i| i["type"].as_str().unwrap())
        .collect();
    assert_eq!(types, vec!["userMessage", "agentMessage"]);
}

#[test]
fn a_real_completed_turn_is_captured_not_held() {
    // The exact record sequence of a credentialed headless turn (Ollama Cloud
    // route, one assistant message, no tool calls), including the rendered
    // system prompt and both title events.
    let value = log(vec![
        record(
            3,
            "agent/inbox/spliced",
            json!({"target":"nextTurn","start":0,"inserted":["msg-8"]}),
        ),
        turn_start(4, 1),
        record(
            5,
            "agent/inbox/spliced",
            json!({"target":"nextStep","start":0,"inserted":["msg-8"]}),
        ),
        record(6, "step/start", json!({"turn":1,"step":1})),
        record(
            7,
            "system/message",
            json!({"turn":1,"step":1,"message":{"id":"system-1","role":"system","content":[{"type":"text","text":"You are a coding agent."}],"source":{"kind":"system-prompt"}}}),
        ),
        human(8, 1, 1),
        injected(9, 1, 1, "agent-instructions"),
        injected(10, 1, 1, "runtime-context"),
        injected(11, 1, 1, "skill-catalog"),
        record(
            12,
            "request/header",
            json!({"header":{},"reason":"initial"}),
        ),
        record(
            13,
            "request/context",
            json!({"provider":"ollama-cloud","model":"deepseek-v4.1-flash:1017","contextWindow":131072}),
        ),
        record(
            14,
            "session/title",
            json!({"kind":"fallback","title":"Reply with the single word: ok","messageSeqs":[8]}),
        ),
        record(15, "session/title-llm-request", json!({"kind":"request"})),
        record(
            16,
            "session/title",
            json!({"kind":"provider","title":"Single-word reply","messageSeqs":[8]}),
        ),
        record(
            17,
            "assistant/message",
            json!({"turn":1,"step":1,"message":{"id":"assistant-1","role":"assistant","content":[{"type":"text","text":"ok"}],"source":{"kind":"model"}}}),
        ),
        record(18, "step/end", json!({"turn":1,"step":1})),
        turn_end(19, 1, json!({"kind":"completed"})),
    ]);
    let Capture::Ready(bundle) = decode(&value, 1) else {
        panic!("a real completed turn must be captured");
    };
    assert_eq!(bundle.outcome, "completed");
    let types: Vec<_> = bundle
        .items
        .iter()
        .map(|i| i["type"].as_str().unwrap())
        .collect();
    assert_eq!(types, vec!["userMessage", "agentMessage"]);
}

#[test]
fn a_capture_after_a_successful_compaction_still_publishes_the_turn() {
    // The projection the bridge writes once `/compact` has finished: the
    // bracket is closed, the summary checkpoint replaces the earlier surface,
    // and the turn's own records are untouched.
    let value = log(vec![
        turn_start(3, 1),
        real_human(4),
        record(
            5,
            "assistant/message",
            json!({"turn":1,"step":1,"message":{"id":"a-1","role":"assistant","content":[{"type":"text","text":"ok"}],"source":{"kind":"model","provider":"p","model":"m"}}}),
        ),
        turn_end(6, 1, json!({"kind": "completed"})),
        record(
            7,
            "command/run",
            json!({"commandId":"cmd-1","name":"compact","args":"","source":{"kind":"user"}}),
        ),
        record(
            8,
            "compaction/start",
            json!({"compactionId":"c-1","sourceCommandId":"cmd-1","turn":null}),
        ),
        record(
            9,
            "compaction/summary",
            json!({"compactionId":"c-1","summary":[{"type":"text","text":"summary"}],"shadowedSeqs":[4],"shadowedTokenCount":10}),
        ),
        record(
            10,
            "user/message",
            json!({"id":"checkpoint","role":"user","content":[{"type":"text","text":"checkpoint"}],"source":{"kind":"compaction"}}),
        ),
        record(
            11,
            "compaction/end",
            json!({"compactionId":"c-1","sourceCommandId":"cmd-1","turn":null}),
        ),
        record(
            12,
            "command/done",
            json!({"commandId":"cmd-1","kind":"success","text":"Compacted 1 history items"}),
        ),
    ]);
    // The checkpoint carries the replacement op and has no turn of its own.
    let mut value = value;
    value["records"][9]["surfaceOp"] = json!({"op":"replace","startSeq":4,"endSeq":4});
    let Capture::Ready(bundle) = decode(&value, 1) else {
        panic!("the turn is captured from the compacted projection");
    };
    let types: Vec<_> = bundle
        .items
        .iter()
        .map(|i| i["type"].as_str().unwrap())
        .collect();
    assert_eq!(types, vec!["userMessage", "agentMessage"]);
}

#[test]
fn a_compaction_request_after_the_turn_does_not_hold_or_join_it() {
    // `/compact` appends `command/run` and a `compaction/*` bracket after the
    // turn ended. They carry no turn number and are not the turn's work.
    let value = log(vec![
        turn_start(3, 1),
        human(4, 1, 1),
        turn_end(5, 1, json!({"kind": "completed"})),
        record(
            6,
            "command/run",
            json!({"commandId":"cmd-1","name":"compact","args":"","source":{"kind":"user"}}),
        ),
        record(
            7,
            "compaction/start",
            json!({"compactionId":"c-1","sourceCommandId":"cmd-1","turn":null}),
        ),
        record(
            8,
            "compaction/end",
            json!({"compactionId":"c-1","sourceCommandId":"cmd-1","turn":null,"error":"No compactable history yet"}),
        ),
    ]);
    let Capture::Ready(bundle) = decode(&value, 1) else {
        panic!("the completed turn is still capturable");
    };
    let types: Vec<_> = bundle
        .items
        .iter()
        .map(|i| i["type"].as_str().unwrap())
        .collect();
    assert_eq!(types, vec!["userMessage"]);
}

/// Messages as DSH actually records them: no `turn` field on a message.
fn real_human(seq: u64) -> Value {
    json!({"seq": seq, "type": "user/message", "time": 1000 + seq, "surfaceOp": "append",
        "data": {"id": format!("msg-{seq}"), "role": "user",
                 "content": [{"type": "text", "text": "do work"}], "source": {"kind": "user"}}})
}
fn real_injected(seq: u64, kind: &str) -> Value {
    json!({"seq": seq, "type": "user/message", "time": 1000 + seq, "surfaceOp": "append",
        "data": {"id": format!("msg-{seq}"), "role": "user",
                 "content": [{"type": "text", "text": "context"}], "source": {"kind": kind}}})
}

#[test]
fn messages_without_a_turn_field_belong_to_the_enclosing_turn() {
    // Discovery and capture must attribute by turn/start, not by a message
    // field: DSH records none, so attribute-scanning made every real turn
    // look non-human (`history_missing`).
    let value = log(vec![
        turn_start(3, 1),
        real_injected(4, "time-context"),
        real_human(5),
        record(
            6,
            "assistant/message",
            json!({"turn":1,"step":1,"message":{"id":"a-1","role":"assistant","content":[{"type":"text","text":"ok"}],"source":{"kind":"model","provider":"p","model":"m"}}}),
        ),
        turn_end(7, 1, json!({"kind":"completed"})),
        turn_start(8, 2),
        real_human(9),
        turn_end(10, 2, json!({"kind":"completed"})),
    ]);
    let Capture::Ready(first) = decode(&value, 1) else {
        panic!("turn 1 is a human run");
    };
    let Capture::Ready(second) = decode(&value, 2) else {
        panic!("turn 2 is a human run");
    };
    assert_eq!(first.items.len(), 2, "only the human message and answer");
    assert_eq!(second.items.len(), 1);
    let dir = tempdir().unwrap();
    let path = dir.path().join("session.json");
    std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    let runs = dsh::discover(&path, "/repo", "storedigest").expect("discover");
    assert_eq!(
        runs.iter()
            .map(|r| r.turn.turn.as_str())
            .collect::<Vec<_>>(),
        vec!["1", "2"]
    );
}

#[test]
fn items_carry_a_citable_id_and_the_shared_vocabulary() {
    let Capture::Ready(bundle) = decode(&two_turn_log(), 1) else {
        panic!("expected a ready capture");
    };
    for item in &bundle.items {
        assert!(
            item["id"].as_str().is_some_and(|id| !id.is_empty()),
            "extraction builds its citation enum from a top-level id: {item}"
        );
    }
    let mut ids: Vec<_> = bundle
        .items
        .iter()
        .filter_map(|i| i["id"].as_str())
        .collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), bundle.items.len(), "ids are unique");
}

#[test]
fn record_times_are_compared_in_seconds() {
    // DSH commits milliseconds; an enablement epoch is seconds. Comparing the
    // units directly made every pre-registration check false.
    let epoch_second = 1_700_000_000;
    let before = log(vec![
        turn_start(3, 1),
        real_human(4),
        turn_end(5, 1, json!({"kind": "completed"})),
    ]);
    let mut value = before;
    for record in value["records"].as_array_mut().unwrap() {
        record["time"] = json!(epoch_second * 1000);
    }
    assert!(
        matches!(decode_at(&value, 1, epoch_second), Capture::Excluded),
        "a turn ending at the epoch second predates enablement"
    );
    let mut later = value.clone();
    for record in later["records"].as_array_mut().unwrap() {
        record["time"] = json!((epoch_second + 1) * 1000);
    }
    assert!(matches!(
        decode_at(&later, 1, epoch_second),
        Capture::Ready(_)
    ));
}

#[test]
fn a_projection_must_declare_a_supported_bridge_schema() {
    let mut value = two_turn_log();
    value.as_object_mut().unwrap().remove("schema");
    assert!(matches!(
        decode(&value, 1),
        Capture::Unsupported(Reason::ProjectionSchema)
    ));
    value["schema"] = json!(2);
    assert!(matches!(
        decode(&value, 1),
        Capture::Unsupported(Reason::ProjectionSchema)
    ));
}

#[test]
fn the_same_turn_decodes_to_the_same_evidence() {
    let (first, second) = (decode(&two_turn_log(), 1), decode(&two_turn_log(), 1));
    let (Capture::Ready(a), Capture::Ready(b)) = (first, second) else {
        panic!("ready");
    };
    assert_eq!(
        serde_json::to_vec(&a).unwrap(),
        serde_json::to_vec(&b).unwrap()
    );
    assert_eq!(
        a.run,
        coding_memory::domain::RunKey {
            store: "storedigest".into(),
            thread: "session-a".into(),
            turn: "1".into()
        }
    );
}

#[test]
fn non_human_turns_are_excluded() {
    let only_injected = log(vec![
        turn_start(3, 1),
        injected(4, 1, 1, "skill-catalog"),
        turn_end(5, 1, json!({"kind": "completed"})),
    ]);
    assert!(matches!(decode(&only_injected, 1), Capture::Excluded));
}

#[test]
fn forked_sessions_exclude_their_inherited_prefix() {
    let mut value = two_turn_log();
    value["header"]["isSeeded"] = json!(true);
    value["records"]
        .as_array_mut()
        .unwrap()
        .insert(3, record(100, "session/end-seed", json!({})));

    // Turn 1 ends inside the inherited prefix: never captured from the child.
    assert!(matches!(decode(&value, 1), Capture::Excluded));

    // Turn 2 owns its human message, but the inherited context would make the
    // evidence a partial view; fork capture is out of scope.
    assert!(matches!(decode(&value, 2), Capture::Excluded));
}

#[test]
fn a_resumed_session_still_captures_its_own_turns() {
    // Observed on `agents.resume`: the resumed process appends
    // `session/end-seed` while the header keeps `isSeeded: false`. Treating that
    // marker as a fork would silently drop every turn of every resumed session.
    let value = log(vec![
        turn_start(3, 1),
        real_human(4),
        turn_end(5, 1, json!({"kind": "completed"})),
        record(6, "session/end-seed", json!({})),
        turn_start(7, 2),
        real_human(8),
        turn_end(9, 2, json!({"kind": "completed"})),
    ]);
    let Capture::Ready(bundle) = decode(&value, 2) else {
        panic!("the resumed turn is captured");
    };
    assert_eq!(bundle.run.turn, "2");
    let dir = tempdir().unwrap();
    let path = dir.path().join("session.json");
    std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    let projection = dsh::Projection::parse(&std::fs::read(&path).unwrap()).expect("parse");
    assert!(projection.resumed());
    assert_eq!(projection.completed_runs().len(), 2);
}

#[test]
fn subagent_children_are_excluded() {
    let mut value = two_turn_log();
    value["header"]["origin"] = json!("subagent");
    assert!(matches!(decode(&value, 1), Capture::Excluded));
    value["header"]["origin"] = serde_json::Value::Null;
    value["header"]["delegationDepth"] = json!(1);
    assert!(matches!(decode(&value, 1), Capture::Excluded));
}

#[test]
fn end_reasons_map_to_the_closed_outcome_set() {
    let cases = [
        ("completed", Some("completed")),
        ("error", Some("failed")),
        ("aborted", Some("interrupted")),
        ("interrupted", Some("interrupted")),
        ("blocked", None),
        ("max-tokens", None),
        ("forked", None),
        ("plugin-specific", None),
    ];
    for (kind, outcome) in cases {
        let value = log(vec![
            turn_start(3, 1),
            human(4, 1, 1),
            turn_end(5, 1, json!({"kind": kind})),
        ]);
        match (decode(&value, 1), outcome) {
            (Capture::Ready(bundle), Some(expected)) => assert_eq!(bundle.outcome, expected),
            (Capture::Unsupported(Reason::TurnEndReason), None) => {}
            (other, _) => panic!("{kind}: unexpected {other:?}"),
        }
    }
}

#[test]
fn an_open_turn_is_pending_and_a_missing_turn_is_missing() {
    let open = log(vec![turn_start(3, 1), human(4, 1, 1)]);
    assert!(matches!(decode(&open, 1), Capture::Pending));
    assert!(matches!(decode(&two_turn_log(), 9), Capture::Missing));
}

#[test]
fn unknown_event_types_and_foreign_formats_are_held() {
    let unknown = log(vec![
        turn_start(3, 1),
        human(4, 1, 1),
        record(5, "future/plugin-event", json!({"x": 1})),
        turn_end(6, 1, json!({"kind": "completed"})),
    ]);
    assert!(matches!(
        decode(&unknown, 1),
        Capture::Unsupported(Reason::UnknownEventType)
    ));

    let ignorable = log(vec![
        turn_start(3, 1),
        human(4, 1, 1),
        {
            let mut unknown = record(5, "future/plugin-event", json!({"x": 1}));
            unknown["ignorable"] = json!(true);
            unknown
        },
        turn_end(6, 1, json!({"kind": "completed"})),
    ]);
    let mut value = ignorable;
    value["records"][5]["ignorable"] = json!(true);
    assert!(matches!(
        decode(&value, 1),
        Capture::Unsupported(Reason::UnknownEventType)
    ));

    let mut foreign = two_turn_log();
    foreign["header"]["version"] = json!(5);
    assert!(matches!(
        decode(&foreign, 1),
        Capture::Unsupported(Reason::HistoryVersion)
    ));
}

#[test]
fn broken_sequences_identities_and_ownership_are_rejected() {
    let mut gap = two_turn_log();
    gap["records"][4]["seq"] = json!(99);
    assert!(matches!(
        decode(&gap, 1),
        Capture::Unsupported(Reason::Sequence)
    ));

    // Identity collisions inside a turn are held, not merged: a record that
    // reuses another record's identity is a source defect.
    let duplicated = log(vec![
        turn_start(3, 1),
        human(4, 1, 1),
        record(
            5,
            "tool/call",
            json!({"turn":1,"step":1,"callId":"c1","name":"read","arguments":{}}),
        ),
        record(
            6,
            "tool/call",
            json!({"turn":1,"step":1,"callId":"c1","name":"write","arguments":{}}),
        ),
        turn_end(7, 1, json!({"kind":"completed"})),
    ]);
    assert!(matches!(
        decode(&duplicated, 1),
        Capture::Unsupported(Reason::DuplicateIdentity)
    ));

    let mut wrong_project = two_turn_log();
    wrong_project["header"]["cwd"] = json!("/other");
    assert!(matches!(decode(&wrong_project, 1), Capture::WrongProject));

    let mut ambiguous = two_turn_log();
    ambiguous["records"][4]["data"]["turn"] = json!(2);
    assert!(matches!(
        decode(&ambiguous, 1),
        Capture::Unsupported(Reason::TurnAttribution)
    ));
}

#[test]
fn oversized_turns_are_over_limit() {
    let big = "x".repeat(coding_memory::dsh::MAX_BYTES);
    let value = log(vec![
        turn_start(3, 1),
        human(4, 1, 1),
        record(
            5,
            "tool/result",
            json!({"turn":1,"step":1,"message":{"id":"t1","role":"tool","toolCallId":"c1","content":[{"type":"text","text":big}]},"sourceEventSeqs":[4]}),
        ),
        turn_end(6, 1, json!({"kind": "completed"})),
    ]);
    let oversized = dsh::decode(&value, "storedigest", "/repo", 1, 3).expect("decode");
    assert!(matches!(oversized, Capture::OverLimit));
}

#[test]
fn discovery_lists_only_completed_human_turns_of_the_project() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("session.json");
    std::fs::write(&path, serde_json::to_vec(&two_turn_log()).unwrap()).unwrap();
    let runs = dsh::discover(&path, "/repo", "storedigest").expect("discover");
    let turns: Vec<_> = runs.iter().map(|r| r.turn.turn.as_str()).collect();
    assert_eq!(turns, vec!["1", "2"]);
    assert!(runs.iter().all(|r| r.turn.thread == "session-a"));

    let other = dsh::discover(&path, "/elsewhere", "storedigest").expect("discover");
    assert!(other.is_empty());
}

#[test]
fn enqueue_signal_requires_known_turn_number_and_cwd() {
    let signal = json!({
        "kind": "turn-end",
        "session": "session-a",
        "turn": 3,
        "cwd": "/repo",
        "reason": "completed"
    });
    let (key, cwd) = dsh::signal(&signal, "storedigest").expect("signal");
    assert_eq!(cwd, "/repo");
    assert_eq!(key.store, "storedigest");
    assert_eq!(key.thread, "session-a");
    assert_eq!(key.turn, "3");

    let mut missing = signal.clone();
    missing["turn"] = json!(0);
    assert!(dsh::signal(&missing, "storedigest").is_err());
    let mut bad_kind = signal;
    bad_kind["kind"] = json!("turn-start");
    assert!(dsh::signal(&bad_kind, "storedigest").is_err());
}

mod ingestion {
    use coding_memory::{
        domain::RunKey,
        ingestion::{self, DshOperation},
        state::{Enqueue, State},
    };
    use serde_json::{Value, json};
    use std::{path::Path, process::Command};
    use tempfile::tempdir;

    /// A registered Git project and its canonical id.
    fn project(dir: &Path) -> (std::path::PathBuf, String) {
        let repo = dir.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        Command::new("git")
            .args(["init", "-q"])
            .arg(&repo)
            .status()
            .unwrap();
        let p = coding_memory::project::resolve(&repo).unwrap();
        (repo.canonicalize().unwrap(), p.id)
    }
    fn session(path: &Path, cwd: &str, turns: u64) {
        let mut records =
            vec![json!({"seq":0,"type":"permission/preset","time":1,"data":{"preset":"default"}})];
        let mut seq = 1;
        for turn in 1..=turns {
            records.push(
                json!({"seq":seq,"type":"turn/start","time":1000+turn*10,"data":{"turn":turn}}),
            );
            seq += 1;
            records.push(
                json!({"seq":seq,"type":"user/message","time":1001+turn*10,"data":{
                "turn":turn,"step":1,"id":format!("m{turn}"),"role":"user",
                "content":[{"type":"text","text":"do work"}],"source":{"kind":"user"}}}),
            );
            seq += 1;
            records.push(json!({"seq":seq,"type":"turn/end","time":1002+turn*10,"data":{"turn":turn,"reason":{"kind":"completed"}}}));
            seq += 1;
        }
        let document = json!({"schema":1,"header":{
            "type":"session","version":4,"id":"session-x","createdAt":1000,"cwd":cwd,"isSeeded":false},
            "records":records});
        std::fs::write(path, serde_json::to_vec(&document).unwrap()).unwrap();
    }
    fn runtime() -> (tempfile::TempDir, tempfile::TempDir) {
        (tempdir().unwrap(), tempdir().unwrap())
    }

    #[tokio::test]
    async fn register_reconcile_and_capture_publish_evidence() -> anyhow::Result<()> {
        let (data, dsh) = runtime();
        let (repo, id) = project(data.path());
        let root = data.path().join("state");
        std::fs::create_dir_all(&root)?;
        let mut state = State::open(&root.join("state.sqlite"))?;
        let store = ingestion::store_id(dsh.path())?;
        let projection = data.path().join("session.json");
        let cwd = repo.to_str().unwrap().to_string();
        session(&projection, &cwd, 2);
        let epoch = ingestion::now();

        // Baseline: both existing turns pre-date enablement.
        let report = ingestion::dsh(
            &mut state,
            &store,
            &root,
            &DshOperation::Register {
                session: projection.clone(),
                project: repo.clone(),
            },
            None,
        )
        .await?;
        assert_eq!(report.baseline, Some(2));
        assert!(state.project_epoch(&id)? >= epoch);
        let report = ingestion::dsh(
            &mut state,
            &store,
            &root,
            &DshOperation::Reconcile {
                session: projection.clone(),
                project: repo.clone(),
            },
            None,
        )
        .await?;
        assert_eq!(report.queued, Some(0), "baseline turns are never queued");
        assert!(
            state.status()?.is_empty(),
            "reconcile does not enqueue excluded turns"
        );

        // A turn a live process saw end after registration is queued via its
        // hint, then captured. Reconciliation alone never queues a run.
        session(&projection, &cwd, 3);
        let mut document: Value = serde_json::from_slice(&std::fs::read(&projection)?)?;
        for record in document["records"].as_array_mut().unwrap() {
            let seq = record["seq"].as_u64().unwrap();
            // DSH commits milliseconds; keep the turn after the epoch second.
            record["time"] = json!(epoch * 1000 + 1000 + seq as i64);
        }
        std::fs::write(&projection, serde_json::to_vec(&document)?)?;
        let report = ingestion::dsh(
            &mut state,
            &store,
            &root,
            &DshOperation::Reconcile {
                session: projection.clone(),
                project: repo.clone(),
            },
            None,
        )
        .await?;
        assert_eq!(report.queued, Some(0), "reconcile is not a capture source");
        // The harness only flushes later, so make turn 3 end after enablement
        // (milliseconds, as DSH commits them) and re-read the projection.
        let epoch = state.project_epoch(&id)?;
        let mut document: Value = serde_json::from_slice(&std::fs::read(&projection)?)?;
        for record in document["records"].as_array_mut().unwrap() {
            if record["data"]["turn"] == json!(3) {
                let seq = record["seq"].as_u64().unwrap();
                record["time"] = json!((epoch + 1) * 1000 + seq as i64);
            }
        }
        std::fs::write(&projection, serde_json::to_vec(&document)?)?;
        let signal = json!({"kind":"turn-end","session":"session-x","turn":3,"cwd":cwd});
        let report = ingestion::dsh(
            &mut state,
            &store,
            &root,
            &DshOperation::Enqueue,
            Some(signal.to_string()),
        )
        .await?;
        assert_eq!(report.state, Some("queued"), "the live hint queues its run");

        let report = ingestion::dsh(
            &mut state,
            &store,
            &root,
            &DshOperation::Capture {
                session: projection.clone(),
                project: repo.clone(),
                turn: None,
            },
            None,
        )
        .await?;
        assert_eq!(report.state, Some("ready"), "{report:?}");
        let digest = report.evidence.expect("evidence digest");
        let evidence = coding_memory::evidence::load(&root, &digest)?;
        assert_eq!(evidence.outcome, "completed");
        assert_eq!(evidence.project, id);
        assert_eq!(evidence.run.turn, "3");
        assert_eq!(
            evidence.items.len(),
            1,
            "only the human message is evidence"
        );

        // Replaying the same session never recaptures a finished turn.
        let report = ingestion::dsh(
            &mut state,
            &store,
            &root,
            &DshOperation::Capture {
                session: projection,
                project: repo,
                turn: None,
            },
            None,
        )
        .await?;
        assert_eq!(report.reason.as_deref(), Some("no_ready_run"));
        Ok(())
    }

    #[tokio::test]
    async fn a_run_no_live_process_saw_end_is_never_captured() -> anyhow::Result<()> {
        // Reconciliation can queue a run whose end this process never observed.
        // Publishing it from a session read would capture the wrong run, so
        // capture refuses without a hint.
        let (data, dsh) = runtime();
        let (repo, id) = project(data.path());
        let root = data.path().join("state");
        std::fs::create_dir_all(&root)?;
        let mut state = State::open(&root.join("state.sqlite"))?;
        let store = ingestion::store_id(dsh.path())?;
        let projection = data.path().join("session.json");
        let cwd = repo.to_str().unwrap().to_string();
        // Register while the session has one completed turn: that turn is the
        // enablement baseline. The turn to capture arrives afterwards.
        session(&projection, &cwd, 1);
        ingestion::dsh(
            &mut state,
            &store,
            &root,
            &DshOperation::Register {
                session: projection.clone(),
                project: repo.clone(),
            },
            None,
        )
        .await?;
        // Second-resolution boundary: the new turn starts in a later second.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let epoch = ingestion::now();
        session(&projection, &cwd, 2);
        let mut document: Value = serde_json::from_slice(&std::fs::read(&projection)?)?;
        for record in document["records"].as_array_mut().unwrap() {
            let seq = record["seq"].as_u64().unwrap();
            // DSH commits milliseconds; keep the turn after the epoch second.
            record["time"] = json!(epoch * 1000 + 1000 + seq as i64);
        }
        std::fs::write(&projection, serde_json::to_vec(&document)?)?;
        // Exactly what reconciliation produces: a queued run with no hint.
        let key = RunKey {
            store: store.clone(),
            thread: "session-x".into(),
            turn: "2".into(),
        };
        state.enqueue(&key, &id, &cwd, 0)?;
        let report = ingestion::dsh(
            &mut state,
            &store,
            &root,
            &DshOperation::Capture {
                session: projection,
                project: repo,
                turn: None,
            },
            None,
        )
        .await?;
        assert_eq!(report.reason.as_deref(), Some("no_capture_hint"));
        assert!(report.evidence.is_none(), "no evidence is published");
        assert_eq!(report.run.as_ref().map(|r| r.turn.as_str()), Some("2"));
        assert_eq!(
            state.status()?[0].state,
            "held",
            "the run stays inspectable rather than silently skipped"
        );
        Ok(())
    }

    #[tokio::test]
    async fn a_hint_recorded_before_registration_resolves_after_it() -> anyhow::Result<()> {
        // A live run can end before its project is registered. The hint names
        // the exact run that ended, so reconcile queues it once registration
        // enables the project instead of dropping it.
        let (data, dsh) = runtime();
        let (repo, id) = project(data.path());
        let root = data.path().join("state");
        std::fs::create_dir_all(&root)?;
        let mut state = State::open(&root.join("state.sqlite"))?;
        let store = ingestion::store_id(dsh.path())?;
        let projection = data.path().join("session.json");
        let cwd = repo.to_str().unwrap().to_string();
        session(&projection, &cwd, 1);
        let signal = json!({"kind":"turn-end","session":"session-x","turn":1,"cwd":cwd});
        let report = ingestion::dsh(
            &mut state,
            &store,
            &root,
            &DshOperation::Enqueue,
            Some(signal.to_string()),
        )
        .await?;
        assert_eq!(
            report.state,
            Some("deferred"),
            "the hint is kept, not dropped"
        );
        assert_eq!(state.pending_hints(&store)?.len(), 1);

        // The harness only flushes on teardown, so the run ends in an earlier
        // second than the registration that enables the project.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        ingestion::dsh(
            &mut state,
            &store,
            &root,
            &DshOperation::Register {
                session: projection.clone(),
                project: repo.clone(),
            },
            None,
        )
        .await?;
        let report = ingestion::dsh(
            &mut state,
            &store,
            &root,
            &DshOperation::Reconcile {
                session: projection.clone(),
                project: repo.clone(),
            },
            None,
        )
        .await?;
        assert_eq!(
            report.queued,
            Some(0),
            "a turn ending before enablement is excluded"
        );
        assert!(
            state.pending_hints(&store)?.is_empty(),
            "the hint is resolved"
        );

        // A run that ends after registration resolves into its exact run.
        let epoch = ingestion::now();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        session(&projection, &cwd, 2);
        let mut document: Value = serde_json::from_slice(&std::fs::read(&projection)?)?;
        for record in document["records"].as_array_mut().unwrap() {
            let seq = record["seq"].as_u64().unwrap();
            // DSH commits milliseconds; keep the turn after the epoch second.
            record["time"] = json!(epoch * 1000 + 1000 + seq as i64);
        }
        std::fs::write(&projection, serde_json::to_vec(&document)?)?;
        let signal = json!({"kind":"turn-end","session":"session-x","turn":2,"cwd":cwd});
        ingestion::dsh(
            &mut state,
            &store,
            &root,
            &DshOperation::Enqueue,
            Some(signal.to_string()),
        )
        .await?;
        assert_eq!(
            state.enqueue_hinted(
                &RunKey {
                    store: store.clone(),
                    thread: "session-x".into(),
                    turn: "2".into()
                },
                &id,
                &cwd,
                0
            )?,
            Enqueue::Duplicate,
            "the hinted run is already queued"
        );
        Ok(())
    }

    /// A projection for `session` with one completed human turn after `time_ms`.
    fn projection_for(path: &Path, cwd: &str, session: &str, time_ms: i64) {
        let document = json!({"schema":1,"header":{
        "type":"session","version":4,"id":session,"createdAt":time_ms,"cwd":cwd,"isSeeded":false},
        "records":[
            {"seq":0,"type":"turn/start","time":time_ms,"data":{"turn":1}},
            {"seq":1,"type":"user/message","time":time_ms+1,"surfaceOp":"append","data":{
                "id":"m1","role":"user","content":[{"type":"text","text":"work"}],"source":{"kind":"user"}}},
            {"seq":2,"type":"assistant/message","time":time_ms+2,"surfaceOp":"append","data":{
                "turn":1,"step":1,"message":{"id":"a1","role":"assistant","content":[{"type":"text","text":"done"}],
                "source":{"kind":"model","provider":"p","model":"m"}}}},
            {"seq":3,"type":"turn/end","time":time_ms+3,"data":{"turn":1,"reason":{"kind":"completed"}}},
        ]});
        std::fs::write(path, serde_json::to_vec(&document).unwrap()).unwrap();
    }

    #[tokio::test]
    async fn capture_claims_the_session_its_projection_describes() -> anyhow::Result<()> {
        // Two sessions of one store queued at once: a capture must publish the
        // projection's own session, never the store's oldest run.
        let (data, dsh) = runtime();
        let (repo, id) = project(data.path());
        let root = data.path().join("state");
        std::fs::create_dir_all(&root)?;
        let mut state = State::open(&root.join("state.sqlite"))?;
        let store = ingestion::store_id(dsh.path())?;
        let cwd = repo.to_str().unwrap().to_string();
        let old = data.path().join("old.json");
        projection_for(&old, &cwd, "session-old", 1);
        ingestion::dsh(
            &mut state,
            &store,
            &root,
            &DshOperation::Register {
                session: old.clone(),
                project: repo.clone(),
            },
            None,
        )
        .await?;
        let later = (ingestion::now() + 5) * 1000;
        for (session, path) in [
            ("session-a", data.path().join("a.json")),
            ("session-b", data.path().join("b.json")),
        ] {
            projection_for(&path, &cwd, session, later);
            let signal = json!({"kind":"turn-end","session":session,"turn":1,"cwd":cwd});
            ingestion::dsh(
                &mut state,
                &store,
                &root,
                &DshOperation::Enqueue,
                Some(signal.to_string()),
            )
            .await?;
        }
        // Capture session-b first; session-a's older run must stay queued.
        let report = ingestion::dsh(
            &mut state,
            &store,
            &root,
            &DshOperation::Capture {
                session: data.path().join("b.json"),
                project: repo.clone(),
                turn: None,
            },
            None,
        )
        .await?;
        assert_eq!(report.state, Some("ready"), "{report:?}");
        assert_eq!(
            report.run.as_ref().map(|r| r.thread.as_str()),
            Some("session-b")
        );
        let digest = report.evidence.expect("digest");
        let evidence = coding_memory::evidence::load(&root, &digest)?;
        assert_eq!(
            evidence.run.thread, "session-b",
            "evidence belongs to the projection"
        );
        let status = state.status()?;
        assert_eq!(status.len(), 2);
        assert_eq!(
            status
                .iter()
                .find(|s| s.key.thread == "session-a")
                .map(|s| s.state.as_str()),
            Some("queued"),
            "the other session's run is untouched"
        );
        assert_eq!(evidence.project, id);
        Ok(())
    }

    #[tokio::test]
    async fn reconciling_one_session_leaves_another_sessions_hint_pending() -> anyhow::Result<()> {
        let (data, dsh) = runtime();
        let (repo, _) = project(data.path());
        let root = data.path().join("state");
        std::fs::create_dir_all(&root)?;
        let mut state = State::open(&root.join("state.sqlite"))?;
        let store = ingestion::store_id(dsh.path())?;
        let cwd = repo.to_str().unwrap().to_string();
        let a = data.path().join("a.json");
        let b = data.path().join("b.json");
        projection_for(&a, &cwd, "session-a", 1);
        projection_for(&b, &cwd, "session-b", 1);
        // Both hints arrive before registration.
        for session in ["session-a", "session-b"] {
            let signal = json!({"kind":"turn-end","session":session,"turn":1,"cwd":cwd});
            ingestion::dsh(
                &mut state,
                &store,
                &root,
                &DshOperation::Enqueue,
                Some(signal.to_string()),
            )
            .await?;
        }
        ingestion::dsh(
            &mut state,
            &store,
            &root,
            &DshOperation::Register {
                session: a.clone(),
                project: repo.clone(),
            },
            None,
        )
        .await?;
        ingestion::dsh(
            &mut state,
            &store,
            &root,
            &DshOperation::Reconcile {
                session: a,
                project: repo.clone(),
            },
            None,
        )
        .await?;
        let pending = state.pending_hints(&store)?;
        assert_eq!(pending.len(), 1, "session-b's hint is not consumed");
        assert_eq!(pending[0].0.thread, "session-b");
        Ok(())
    }

    #[tokio::test]
    async fn registering_twice_never_baselines_later_turns() -> anyhow::Result<()> {
        let (data, dsh) = runtime();
        let (repo, id) = project(data.path());
        let root = data.path().join("state");
        std::fs::create_dir_all(&root)?;
        let mut state = State::open(&root.join("state.sqlite"))?;
        let store = ingestion::store_id(dsh.path())?;
        let cwd = repo.to_str().unwrap().to_string();
        let path = data.path().join("session.json");
        session(&path, &cwd, 1);
        ingestion::dsh(
            &mut state,
            &store,
            &root,
            &DshOperation::Register {
                session: path.clone(),
                project: repo.clone(),
            },
            None,
        )
        .await?;
        let epoch = state.project_epoch(&id)?;
        let later = (epoch + 5) * 1000;
        projection_for(&path, &cwd, "session-x", later);
        // Register baselines the turns this projection already shows, so the
        // post-enablement run arrives afterwards, as it does in production.
        ingestion::dsh(
            &mut state,
            &store,
            &root,
            &DshOperation::Register {
                session: path.clone(),
                project: repo.clone(),
            },
            None,
        )
        .await?;
        let signal = json!({"kind":"turn-end","session":"session-x","turn":2,"cwd":cwd});
        ingestion::dsh(
            &mut state,
            &store,
            &root,
            &DshOperation::Enqueue,
            Some(signal.to_string()),
        )
        .await?;
        // Re-registering must not baseline the queued post-enablement run.
        let report = ingestion::dsh(
            &mut state,
            &store,
            &root,
            &DshOperation::Register {
                session: path,
                project: repo.clone(),
            },
            None,
        )
        .await?;
        assert_eq!(report.reason.as_deref(), Some("already_registered"));
        assert_eq!(
            state
                .status()?
                .iter()
                .find(|s| s.key.thread == "session-x" && s.key.turn == "2")
                .map(|s| s.state.as_str()),
            Some("queued"),
            "re-registering must not baseline the queued post-enablement run"
        );
        Ok(())
    }

    #[tokio::test]
    async fn a_hint_is_persisted_with_its_run_atomically() -> anyhow::Result<()> {
        let (data, dsh) = runtime();
        let (repo, _) = project(data.path());
        let root = data.path().join("state");
        std::fs::create_dir_all(&root)?;
        let mut state = State::open(&root.join("state.sqlite"))?;
        let store = ingestion::store_id(dsh.path())?;
        let cwd = repo.to_str().unwrap().to_string();
        let path = data.path().join("session.json");
        session(&path, &cwd, 1);
        ingestion::dsh(
            &mut state,
            &store,
            &root,
            &DshOperation::Register {
                session: path,
                project: repo,
            },
            None,
        )
        .await?;
        let project_id = coding_memory::project::resolve(std::path::Path::new(&cwd))?.id;
        let key = RunKey {
            store: store.clone(),
            thread: "session-y".into(),
            turn: "1".into(),
        };
        state.enqueue_hinted(&key, &project_id, &cwd, 0)?;
        assert!(state.hinted(&key)?, "a queued run always carries its hint");
        Ok(())
    }

    #[tokio::test]
    async fn a_missing_signal_project_is_rejected() -> anyhow::Result<()> {
        let (data, dsh) = runtime();
        let root = data.path().join("state");
        std::fs::create_dir_all(&root)?;
        let mut state = State::open(&root.join("state.sqlite"))?;
        let store = ingestion::store_id(dsh.path())?;
        let signal = json!({"kind":"turn-end","session":"s","turn":1,"cwd":"/nonexistent/project"});
        assert!(
            ingestion::dsh(
                &mut state,
                &store,
                &root,
                &DshOperation::Enqueue,
                Some(signal.to_string())
            )
            .await
            .is_err()
        );
        Ok(())
    }

    #[tokio::test]
    async fn exclusion_and_ownership_hold_runs_at_enqueue_time() -> anyhow::Result<()> {
        let (data, dsh) = runtime();
        let (repo, _) = project(data.path());
        let root = data.path().join("state");
        std::fs::create_dir_all(&root)?;
        let mut state = State::open(&root.join("state.sqlite"))?;
        let store = ingestion::store_id(dsh.path())?;
        let projection = data.path().join("session.json");
        let cwd = repo.to_str().unwrap().to_string();
        session(&projection, &cwd, 1);
        ingestion::dsh(
            &mut state,
            &store,
            &root,
            &DshOperation::Register {
                session: projection,
                project: repo,
            },
            None,
        )
        .await?;
        state.own_store(&store)?;
        let signal = json!({"kind":"turn-end","session":"session-x","turn":1,"cwd":cwd});
        let report = ingestion::dsh(
            &mut state,
            &store,
            &root,
            &DshOperation::Enqueue,
            Some(signal.to_string()),
        )
        .await?;
        assert_eq!(
            report.state,
            Some("held"),
            "engine-owned stores never queue"
        );
        assert_eq!(
            state.enqueue(
                &RunKey {
                    store: store.clone(),
                    thread: "session-x".into(),
                    turn: "1".into()
                },
                &coding_memory::project::resolve(std::path::Path::new(&cwd))?.id,
                &cwd,
                0
            )?,
            Enqueue::Excluded
        );
        Ok(())
    }

    #[test]
    fn signal_rejects_oversized_and_malformed_values() {
        let store = "s";
        assert!(coding_memory::dsh::signal(&Value::Null, store).is_err());
        let long = "a".repeat(5000);
        assert!(
            coding_memory::dsh::signal(
                &json!({"kind":"turn-end","session":"s","turn":1,"cwd":long}),
                store
            )
            .is_err()
        );
    }
}
