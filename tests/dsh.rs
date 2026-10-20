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

fn projection(log: &Value) -> dsh::Projection {
    dsh::Projection::from_value(log.clone()).expect("projection")
}

fn decode(log: &Value, turn: u64) -> Capture {
    dsh::decode(&projection(log), "storedigest", "/repo", turn, 3).expect("decode")
}

fn decode_at(log: &Value, turn: u64, epoch: i64) -> Capture {
    dsh::decode(&projection(log), "storedigest", "/repo", turn, epoch).expect("decode")
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
fn developer_messages_use_the_nested_identity() {
    // DSH nests the message identity (`data.message.id`); a top-level id is not
    // part of the developer/message payload. Reading the wrong path held every
    // turn that changed the tool registry.
    let value = log(vec![
        turn_start(3, 1),
        human(4, 1, 1),
        record(
            5,
            "developer/message",
            json!({"turn":1,"step":1,"message":{
                "id":"dev-1","role":"developer",
                "content":[{"type":"text","text":"tools added"}],
                "source":{"kind":"developer"}}}),
        ),
        turn_end(6, 1, json!({"kind": "completed"})),
    ]);
    let Capture::Ready(bundle) = decode(&value, 1) else {
        panic!("a developer message must not hold the turn");
    };
    let item = bundle
        .items
        .iter()
        .find(|item| item["ds_type"] == json!("developer/message"))
        .expect("developer message becomes an item");
    assert_eq!(item["id"], json!("dev-1@5"));
    assert_eq!(item["type"], json!("hookPrompt"));
}

#[test]
fn assistant_stream_and_reasoning_blocks_are_not_evidence() {
    let value = log(vec![
        turn_start(3, 1),
        human(4, 1, 1),
        record(
            5,
            "assistant/message",
            json!({"turn":1,"step":1,
                "message":{"id":"assistant-1","role":"assistant","content":[
                    {"type":"reasoning","text":"hidden reasoning"},
                    {"type":"text","text":"answer"}],
                    "source":{"kind":"model","provider":"p","model":"m"}},
                "stream":[{"type":"reasoning-chunks","time0":1,"index":0,"dt":[1],"texts":["hidden reasoning"]}],
                "usage":{"inputTokens":1,"outputTokens":2}}),
        ),
        turn_end(6, 1, json!({"kind": "completed"})),
    ]);
    let Capture::Ready(bundle) = decode(&value, 1) else {
        panic!("expected a ready capture");
    };
    let item = bundle
        .items
        .iter()
        .find(|item| item["ds_type"] == json!("assistant/message"))
        .expect("assistant message becomes an item");
    assert!(
        item["data"].get("stream").is_none(),
        "the timed model stream never becomes evidence"
    );
    let content = item["data"]["message"]["content"].as_array().unwrap();
    assert_eq!(content.len(), 1);
    assert_eq!(content[0]["type"], json!("text"));
    assert_eq!(
        item["data"]["usage"]["outputTokens"],
        json!(2),
        "token accounting is retained"
    );
}

#[test]
fn a_fork_lineage_header_is_excluded_not_rejected() {
    // The bridge emits `parentSession` whenever DSH records lineage. The header
    // must deserialize, and the session is excluded like every other fork.
    let mut value = two_turn_log();
    value["header"]["parentSession"] = json!("parent-session");
    let bytes = serde_json::to_vec(&value).unwrap();
    dsh::Projection::parse(&bytes).expect("a lineage header is a valid projection");
    assert!(matches!(decode(&value, 1), Capture::Excluded));
}

#[test]
fn the_committed_fixture_decodes_with_the_engine() {
    // The fixture is the observed contract; the engine must accept it rather
    // than only the fixture's own assertions doing so.
    let value: Value = serde_json::from_str(include_str!(
        "../experiments/dsh_capture/fixtures/session-v4.json"
    ))
    .expect("fixture JSON");
    let Ok(Capture::Ready(bundle)) =
        dsh::decode(&projection(&value), "store", "/synthetic/project", 1, 0)
    else {
        panic!("the committed fixture decodes to evidence");
    };
    let types: Vec<_> = bundle
        .items
        .iter()
        .map(|item| item["type"].as_str().unwrap())
        .collect();
    assert_eq!(
        types,
        vec![
            "userMessage",
            "toolCall",
            "commandExecution",
            "agentMessage"
        ]
    );
}

#[test]
fn an_end_before_its_start_is_rejected_within_one_second() {
    // Truncating both instants to seconds would hide the invalid ordering.
    let mut value = log(vec![
        turn_start(3, 1),
        human(4, 1, 1),
        turn_end(5, 1, json!({"kind": "completed"})),
    ]);
    for record in value["records"].as_array_mut().unwrap() {
        match record["type"].as_str() {
            Some("turn/start") => record["time"] = json!(1_500_500),
            Some("turn/end") => record["time"] = json!(1_500_100),
            _ => {}
        }
    }
    assert!(matches!(
        decode(&value, 1),
        Capture::Unsupported(Reason::StartedAt)
    ));
}

#[test]
fn a_bounded_turn_window_without_earlier_records_decodes() {
    // The bridge's bounded projection drops every earlier turn, so the range
    // starts at the captured turn's own native `seq` rather than zero. The
    // adapter reads only that window and must still accept it.
    let value = json!({"schema": 1, "header": header("session-a", "/repo"), "records": [
        turn_start(9, 2),
        human(10, 2, 1),
        turn_end(11, 2, json!({"kind": "completed"})),
    ]});
    let Capture::Ready(bundle) = decode(&value, 2) else {
        panic!("expected a ready capture");
    };
    assert_eq!(bundle.items.len(), 1);
    assert_eq!(bundle.items[0]["seq"], json!(10));
    assert_eq!(bundle.run.turn, "2");
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
    value["records"][10]["surfaceOp"] = json!({"op":"replace","startSeq":4,"endSeq":4});
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
    let runs = dsh::discover(&projection(&value), "/repo", "storedigest").expect("discover");
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
fn a_tool_result_must_answer_a_call_in_the_same_turn() {
    // A result whose call is absent would otherwise become a "completed"
    // commandExecution and count as observed support in extraction.
    let orphan = log(vec![
        turn_start(3, 1),
        real_human(4),
        record(
            5,
            "tool/result",
            json!({"turn":1,"step":1,"message":{"id":"t-orphan","role":"tool","toolCallId":"missing","content":[]}}),
        ),
        turn_end(6, 1, json!({"kind":"completed"})),
    ]);
    assert!(matches!(
        decode(&orphan, 1),
        Capture::Unsupported(Reason::ItemIdentity)
    ));
    // Two results for one call are equally unusable.
    let duplicated = log(vec![
        turn_start(3, 1),
        real_human(4),
        record(
            5,
            "tool/call",
            json!({"turn":1,"step":1,"callId":"c1","name":"read","arguments":{}}),
        ),
        record(
            6,
            "tool/result",
            json!({"turn":1,"step":1,"message":{"id":"t1","role":"tool","toolCallId":"c1","content":[]}}),
        ),
        record(
            7,
            "tool/result",
            json!({"turn":1,"step":1,"message":{"id":"t2","role":"tool","toolCallId":"c1","content":[]}}),
        ),
        turn_end(8, 1, json!({"kind":"completed"})),
    ]);
    assert!(matches!(
        decode(&duplicated, 1),
        Capture::Unsupported(Reason::ItemIdentity)
    ));
    // An empty identity is not a call identity either.
    let empty = log(vec![
        turn_start(3, 1),
        real_human(4),
        record(
            5,
            "tool/call",
            json!({"turn":1,"step":1,"name":"read","arguments":{}}),
        ),
        turn_end(6, 1, json!({"kind":"completed"})),
    ]);
    assert!(matches!(
        decode(&empty, 1),
        Capture::Unsupported(Reason::ItemIdentity)
    ));
}

#[test]
fn a_turn_ending_at_the_boundary_reports_before_enablement() {
    let epoch = 1_700_000_000;
    let mut value = log(vec![
        turn_start(3, 1),
        real_human(4),
        turn_end(5, 1, json!({"kind":"completed"})),
    ]);
    for record in value["records"].as_array_mut().unwrap() {
        record["time"] = json!(epoch * 1000 - 10);
    }
    let records = value["records"].as_array_mut().unwrap();
    let last = records.len() - 1;
    records[last]["time"] = json!(epoch * 1000);
    // The reason must name the boundary, not a fork or a nonhuman run.
    assert!(matches!(
        decode_at(&value, 1, epoch),
        Capture::BeforeEnablement
    ));
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
        matches!(
            decode_at(&value, 1, epoch_second),
            Capture::BeforeEnablement
        ),
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
fn a_turn_ending_later_in_the_registration_second_is_captured() {
    // The enablement epoch has second resolution; a turn that ends 500 ms after
    // it began must not be excluded by truncating its millisecond timestamp.
    let epoch = 1_700_000_000;
    let mut value = log(vec![
        turn_start(3, 1),
        real_human(4),
        turn_end(5, 1, json!({"kind": "completed"})),
    ]);
    for record in value["records"].as_array_mut().unwrap() {
        record["time"] = json!(epoch * 1000 + 400);
    }
    // The fixture's turn/end is the last record.
    {
        let records = value["records"].as_array_mut().unwrap();
        let last = records.len() - 1;
        records[last]["time"] = json!(epoch * 1000 + 900);
    }
    assert!(
        matches!(decode_at(&value, 1, epoch), Capture::Ready(_)),
        "a post-registration turn inside the epoch second is valid work"
    );
    {
        let records = value["records"].as_array_mut().unwrap();
        for record in records.iter_mut() {
            record["time"] = json!(epoch * 1000 - 100);
        }
        let last = records.len() - 1;
        records[last]["time"] = json!(epoch * 1000);
    }
    assert!(
        matches!(decode_at(&value, 1, epoch), Capture::BeforeEnablement),
        "a turn ending exactly at the epoch still predates enablement"
    );
}

#[test]
fn the_exact_registration_instant_is_honoured() {
    // Enabling a project mid-second must not admit a run that ended earlier in
    // that same second, which a whole-second epoch would.
    let mut value = log(vec![
        turn_start(3, 1),
        real_human(4),
        turn_end(5, 1, json!({"kind": "completed"})),
    ]);
    let enabled_ms = 1_700_000_000_900;
    for record in value["records"].as_array_mut().unwrap() {
        record["time"] = json!(enabled_ms - 500);
    }
    {
        let records = value["records"].as_array_mut().unwrap();
        let last = records.len() - 1;
        records[last]["time"] = json!(enabled_ms - 300);
    }
    let outcome = dsh::decode_after(
        &projection(&value),
        "storedigest",
        "/repo",
        1,
        dsh::Boundary::Millis(enabled_ms),
    )
    .expect("decode");
    assert!(
        matches!(outcome, Capture::BeforeEnablement),
        "a run ending before the exact instant predates enablement"
    );
    // The same turn, ending after the instant, is post-enablement.
    {
        let records = value["records"].as_array_mut().unwrap();
        let last = records.len() - 1;
        records[last]["time"] = json!(enabled_ms + 100);
    }
    assert!(matches!(
        dsh::decode_after(
            &projection(&value),
            "storedigest",
            "/repo",
            1,
            dsh::Boundary::Millis(enabled_ms)
        )
        .expect("decode"),
        Capture::Ready(_)
    ));
}

#[test]
fn a_projection_must_declare_a_supported_bridge_schema() {
    let mut value = two_turn_log();
    value.as_object_mut().unwrap().remove("schema");
    assert!(
        dsh::Projection::from_value(value.clone()).is_err(),
        "a missing bridge schema is rejected before decoding"
    );
    value["schema"] = json!(2);
    // A future schema may have an unrelated shape; the revision check must run
    // before deserialization so callers see the actionable reason.
    value["records"] = json!(["not a record"]);
    let error = dsh::Projection::from_value(value).expect_err("a future schema is refused");
    assert!(
        error
            .to_string()
            .contains("Unsupported bridge projection schema"),
        "{error}"
    );
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

    // Attribution applies to bookkeeping too: a record that names another turn
    // proves the window is misattributed, even though the record itself would be
    // skipped. Missing payloads remain harmless (see the next test).
    let misattributed = log(vec![
        turn_start(3, 1),
        human(4, 1, 1),
        record(5, "job/registered", json!({"turn":2,"job":"j"})),
        turn_end(6, 1, json!({"kind":"completed"})),
    ]);
    assert!(matches!(
        decode(&misattributed, 1),
        Capture::Unsupported(Reason::TurnAttribution)
    ));
}

#[test]
fn bookkeeping_records_inside_a_turn_need_no_payload() {
    // Bookkeeping may be omitted from a projection entirely, so its presence
    // without `data` must not hold a turn that is otherwise complete. Requiring
    // an attribution field here would turn a harmless envelope into a defect.
    let value = json!({"schema":1,"header":header("session-a","/repo"),"records":[
        turn_start(3, 1),
        human(4, 1, 1),
        json!({"seq":5,"type":"job/registered","time":1006}),
        turn_end(6, 1, json!({"kind":"completed"})),
    ]});
    let Capture::Ready(bundle) = decode(&value, 1) else {
        panic!("bookkeeping without a payload is skipped, not held");
    };
    assert_eq!(bundle.items.len(), 1);
}

#[test]
fn oversized_turns_are_over_limit() {
    let big = "x".repeat(coding_memory::dsh::MAX_BYTES);
    let value = log(vec![
        turn_start(3, 1),
        human(4, 1, 1),
        record(
            5,
            "tool/call",
            json!({"turn":1,"step":1,"callId":"c1","name":"read","arguments":{}}),
        ),
        record(
            6,
            "tool/result",
            json!({"turn":1,"step":1,"message":{"id":"t1","role":"tool","toolCallId":"c1","content":[{"type":"text","text":big}]},"sourceEventSeqs":[5]}),
        ),
        turn_end(7, 1, json!({"kind": "completed"})),
    ]);
    let oversized = dsh::decode(&projection(&value), "storedigest", "/repo", 1, 3).expect("decode");
    assert!(matches!(oversized, Capture::OverLimit));
}

#[test]
fn discovery_lists_only_completed_human_turns_of_the_project() {
    let value = two_turn_log();
    let runs = dsh::discover(&projection(&value), "/repo", "storedigest").expect("discover");
    let turns: Vec<_> = runs.iter().map(|r| r.turn.turn.as_str()).collect();
    assert_eq!(turns, vec!["1", "2"]);
    assert!(runs.iter().all(|r| r.turn.thread == "session-a"));

    let other = dsh::discover(&projection(&value), "/elsewhere", "storedigest").expect("discover");
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

mod migration {
    use coding_memory::state::State;
    use rusqlite::Connection;
    use tempfile::tempdir;

    #[test]
    fn a_schema_v1_database_gains_the_enabled_ms_column() -> anyhow::Result<()> {
        // `CREATE TABLE IF NOT EXISTS` never alters an existing table, so an
        // upgraded installation needs an explicit column migration; without it
        // every DSH read and every project registration fails on the new column.
        let dir = tempdir()?;
        let path = dir.path().join("state.sqlite");
        {
            let db = Connection::open(&path)?;
            db.execute_batch(
                "CREATE TABLE schema_version(version INTEGER NOT NULL);
                 INSERT INTO schema_version VALUES(1);
                 CREATE TABLE projects(id TEXT PRIMARY KEY, repository TEXT NOT NULL UNIQUE, enabled_at INTEGER NOT NULL);
                 INSERT INTO projects VALUES('p','/repo',10);",
            )?;
        }
        let state = State::open(&path)?;
        assert_eq!(state.project_epoch("p")?, 10);
        assert_eq!(
            state.project_epoch_ms("p")?,
            None,
            "a pre-migration row falls back to its whole-second epoch"
        );
        // A newly registered project records the exact instant.
        state_register(&path)?;
        Ok(())
    }

    #[test]
    fn concurrent_opens_migrate_the_same_database_once() -> anyhow::Result<()> {
        // Bridge subprocesses for overlapping turns can open a just-upgraded
        // database together. An unlocked check-then-alter would make all but one
        // fail with a duplicate-column error before writing their turn hint.
        let dir = tempdir()?;
        let path = dir.path().join("state.sqlite");
        {
            let db = Connection::open(&path)?;
            db.execute_batch(
                "CREATE TABLE schema_version(version INTEGER NOT NULL);
                 INSERT INTO schema_version VALUES(1);
                 CREATE TABLE projects(id TEXT PRIMARY KEY, repository TEXT NOT NULL UNIQUE, enabled_at INTEGER NOT NULL);
                 INSERT INTO projects VALUES('p','/repo',10);",
            )?;
        }
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(6));
        let mut handles = Vec::new();
        for _ in 0..6 {
            let path = path.clone();
            let barrier = barrier.clone();
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                State::open(&path).map(|_| ())
            }));
        }
        for handle in handles {
            handle.join().expect("opening thread panicked")?;
        }
        let state = State::open(&path)?;
        assert_eq!(
            state.project_epoch_ms("p")?,
            None,
            "the migrated row keeps its whole-second fallback"
        );
        Ok(())
    }

    #[test]
    fn registration_records_the_instant_chosen_by_its_caller() -> anyhow::Result<()> {
        // The caller samples the boundary before scanning a baseline; the
        // persisted millisecond value must be that instant, not a later sample.
        let dir = tempdir()?;
        let path = dir.path().join("state.sqlite");
        let mut state = State::open(&path)?;
        let enabled_ms = 1_700_000_000_123;
        assert!(
            state.register_project_at("p", "/repo", enabled_ms / 1000, enabled_ms)?,
            "the first registration inserts"
        );
        assert_eq!(state.project_epoch("p")?, enabled_ms / 1000);
        assert_eq!(state.project_epoch_ms("p")?, Some(enabled_ms));
        assert!(
            !state.register_project_at(
                "p",
                "/repo",
                enabled_ms / 1000 + 10,
                enabled_ms + 10_000
            )?,
            "a loser reports that it did not insert and must not baseline"
        );
        assert_eq!(
            state.project_epoch_ms("p")?,
            Some(enabled_ms),
            "the winning instant stays authoritative"
        );
        Ok(())
    }

    #[test]
    fn a_fresh_database_opens_and_registers() -> anyhow::Result<()> {
        // The migration runs before the schema is created, so it must tolerate
        // a database with no `projects` table at all.
        let dir = tempdir()?;
        let path = dir.path().join("state.sqlite");
        let mut state = State::open(&path)?;
        state.register_project("fresh", "/repo", 1)?;
        assert_eq!(state.project_epoch("fresh")?, 1);
        assert!(state.project_epoch_ms("fresh")?.is_some());
        Ok(())
    }

    fn state_register(path: &std::path::Path) -> anyhow::Result<()> {
        let mut state = State::open(path)?;
        state.register_project("q", "/repo2", 20)?;
        assert!(state.project_epoch_ms("q")?.is_some());
        Ok(())
    }
}

mod ingestion {
    use coding_memory::{
        domain::RunKey,
        ingestion::{self, DshOperation, DshReport},
        state::{Enqueue, State},
    };
    use serde_json::{Value, json};
    use std::{
        path::{Path, PathBuf},
        process::Command,
    };
    use tempfile::{TempDir, tempdir};

    /// A registered Git project, an isolated DSH home, and engine state.
    struct Runtime {
        data: TempDir,
        _dsh: TempDir,
        root: PathBuf,
        repo: PathBuf,
        id: String,
        cwd: String,
        store: String,
        state: State,
    }

    impl Runtime {
        fn new() -> anyhow::Result<Self> {
            let data = tempdir()?;
            let dsh = tempdir()?;
            let repo = data.path().join("repo");
            std::fs::create_dir_all(&repo)?;
            let status = Command::new("git")
                .args(["init", "-q"])
                .arg(&repo)
                .status()?;
            assert!(status.success(), "git init failed");
            let repo = repo.canonicalize()?;
            let id = coding_memory::project::resolve(&repo)?.id;
            let root = data.path().join("state");
            std::fs::create_dir_all(&root)?;
            let state = State::open(&root.join("state.sqlite"))?;
            let store = ingestion::dsh_store_id(dsh.path())?;
            let cwd = repo.to_str().unwrap().to_string();
            Ok(Self {
                data,
                _dsh: dsh,
                root,
                repo,
                id,
                cwd,
                store,
                state,
            })
        }

        fn path(&self, name: &str) -> PathBuf {
            self.data.path().join(name)
        }

        fn key(&self, thread: &str, turn: u64) -> RunKey {
            RunKey {
                store: self.store.clone(),
                thread: thread.into(),
                turn: turn.to_string(),
            }
        }

        fn signal(&self, thread: &str, turn: u64) -> Value {
            json!({
                "kind": "turn-end",
                "session": thread,
                "turn": turn,
                "cwd": self.cwd,
            })
        }

        fn run(&mut self, operation: DshOperation) -> anyhow::Result<DshReport> {
            ingestion::dsh(&mut self.state, &self.store, &self.root, operation)
        }

        fn register(&mut self, session: &Path) -> anyhow::Result<DshReport> {
            self.run(DshOperation::Register {
                session: session.to_path_buf(),
                project: self.repo.clone(),
            })
        }

        fn reconcile(&mut self, session: &Path) -> anyhow::Result<DshReport> {
            self.run(DshOperation::Reconcile {
                session: session.to_path_buf(),
                project: self.repo.clone(),
            })
        }

        fn capture(&mut self, session: &Path, turn: Option<u64>) -> anyhow::Result<DshReport> {
            self.run(DshOperation::Capture {
                session: session.to_path_buf(),
                project: self.repo.clone(),
                turn,
            })
        }

        fn enqueue(&mut self, thread: &str, turn: u64) -> anyhow::Result<DshReport> {
            let signal = self.signal(thread, turn);
            self.enqueue_value(&signal)
        }

        fn enqueue_value(&mut self, signal: &Value) -> anyhow::Result<DshReport> {
            self.run(DshOperation::Enqueue {
                signal: Some(signal.to_string()),
            })
        }

        fn hold_over_limit(&mut self, thread: &str, turn: u64) -> anyhow::Result<DshReport> {
            self.run(DshOperation::HoldOverLimit {
                thread: thread.into(),
                turn,
            })
        }

        fn own_store(&mut self) -> anyhow::Result<DshReport> {
            self.run(DshOperation::OwnStore)
        }

        fn status_of(
            &self,
            thread: &str,
            turn: u64,
        ) -> anyhow::Result<Option<(String, Option<String>)>> {
            let turn = turn.to_string();
            Ok(self
                .state
                .status()?
                .into_iter()
                .find(|status| status.key.thread == thread && status.key.turn == turn)
                .map(|status| (status.state, status.reason)))
        }
    }

    fn document(cwd: &str, id: &str, created_at: i64, records: Vec<Value>) -> Value {
        json!({"schema":1,"header":{
            "type":"session","version":4,"id":id,"createdAt":created_at,"cwd":cwd,"isSeeded":false},
            "records":records})
    }

    fn write_document(path: &Path, document: &Value) {
        std::fs::write(path, serde_json::to_vec(document).unwrap()).unwrap();
    }

    /// A projection for one session id.
    fn session_as(path: &Path, cwd: &str, turns: u64, id: &str) {
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
        write_document(path, &document(cwd, id, 1000, records));
    }

    fn session(path: &Path, cwd: &str, turns: u64) {
        session_as(path, cwd, turns, "session-x");
    }

    /// Rewrite every record's millisecond time as a function of its seq.
    fn set_record_times(path: &Path, time: impl Fn(u64) -> i64) -> anyhow::Result<()> {
        let mut document: Value = serde_json::from_slice(&std::fs::read(path)?)?;
        for record in document["records"].as_array_mut().unwrap() {
            let seq = record["seq"].as_u64().unwrap();
            record["time"] = json!(time(seq));
        }
        std::fs::write(path, serde_json::to_vec(&document)?)?;
        Ok(())
    }

    /// Rewrite one turn's record times.
    fn set_turn_times(path: &Path, turn: u64, time: impl Fn(u64) -> i64) -> anyhow::Result<()> {
        let mut document: Value = serde_json::from_slice(&std::fs::read(path)?)?;
        for record in document["records"].as_array_mut().unwrap() {
            if record["data"]["turn"] == json!(turn) {
                let seq = record["seq"].as_u64().unwrap();
                record["time"] = json!(time(seq));
            }
        }
        std::fs::write(path, serde_json::to_vec(&document)?)?;
        Ok(())
    }

    /// A projection with two completed human turns, both after `time_ms`.
    fn two_turn_projection(path: &Path, cwd: &str, session: &str, time_ms: i64) {
        let mut records = Vec::new();
        let mut seq = 0;
        for turn in 1..=2 {
            records
                .push(json!({"seq":seq,"type":"turn/start","time":time_ms,"data":{"turn":turn}}));
            seq += 1;
            records.push(json!({"seq":seq,"type":"user/message","time":time_ms+turn,"surfaceOp":"append","data":{
                "id":format!("m{turn}"),"role":"user","content":[{"type":"text","text":"work"}],"source":{"kind":"user"}}}));
            seq += 1;
            records.push(json!({"seq":seq,"type":"assistant/message","time":time_ms+turn,"surfaceOp":"append","data":{
                "turn":turn,"step":1,"message":{"id":format!("a{turn}"),"role":"assistant","content":[{"type":"text","text":"done"}],
                "source":{"kind":"model","provider":"p","model":"m"}}}}));
            seq += 1;
            records.push(json!({"seq":seq,"type":"turn/end","time":time_ms+turn,"data":{"turn":turn,"reason":{"kind":"completed"}}}));
            seq += 1;
        }
        write_document(path, &document(cwd, session, time_ms, records));
    }

    /// A projection for `session` with one completed human turn after `time_ms`.
    fn projection_for(path: &Path, cwd: &str, session: &str, time_ms: i64) {
        write_document(
            path,
            &document(
                cwd,
                session,
                time_ms,
                vec![
                    json!({"seq":0,"type":"turn/start","time":time_ms,"data":{"turn":1}}),
                    json!({"seq":1,"type":"user/message","time":time_ms+1,"surfaceOp":"append","data":{
                        "id":"m1","role":"user","content":[{"type":"text","text":"work"}],"source":{"kind":"user"}}}),
                    json!({"seq":2,"type":"assistant/message","time":time_ms+2,"surfaceOp":"append","data":{
                        "turn":1,"step":1,"message":{"id":"a1","role":"assistant","content":[{"type":"text","text":"done"}],
                        "source":{"kind":"model","provider":"p","model":"m"}}}}),
                    json!({"seq":3,"type":"turn/end","time":time_ms+3,"data":{"turn":1,"reason":{"kind":"completed"}}}),
                ],
            ),
        );
    }

    /// A one-turn projection whose start/end are exact millisecond instants.
    fn timed_projection(path: &Path, cwd: &str, turn: u64, start_ms: i64, end_ms: i64) {
        write_document(
            path,
            &document(
                cwd,
                "session-x",
                start_ms,
                vec![
                    json!({"seq":0,"type":"turn/start","time":start_ms,"data":{"turn":turn}}),
                    json!({"seq":1,"type":"user/message","time":start_ms+1,"data":{
                        "id":format!("m{turn}"),"role":"user",
                        "content":[{"type":"text","text":"work"}],"source":{"kind":"user"}}}),
                    json!({"seq":2,"type":"turn/end","time":end_ms,"data":{"turn":turn,"reason":{"kind":"completed"}}}),
                ],
            ),
        );
    }

    /// Make a deferred run claimable again, as a later worker cycle would.
    fn make_available(root: &Path, thread: &str) -> anyhow::Result<()> {
        let db = rusqlite::Connection::open(root.join("state.sqlite"))?;
        db.execute("UPDATE runs SET available_at=0 WHERE thread=?1", [thread])?;
        Ok(())
    }

    #[test]
    fn register_reconcile_and_capture_publish_evidence() -> anyhow::Result<()> {
        let mut rt = Runtime::new()?;
        let projection = rt.path("session.json");
        session(&projection, &rt.cwd, 2);
        let epoch = ingestion::now();

        // Baseline: both existing turns pre-date enablement.
        let report = rt.register(&projection)?;
        assert_eq!(report.baseline, Some(2));
        assert!(rt.state.project_epoch(&rt.id)? >= epoch);
        let report = rt.reconcile(&projection)?;
        assert_eq!(report.queued, Some(0), "baseline turns are never queued");
        assert!(
            rt.state.status()?.is_empty(),
            "reconcile does not enqueue excluded turns"
        );

        // A turn a live process saw end after registration is queued via its
        // hint, then captured. Reconciliation alone never queues a run.
        session(&projection, &rt.cwd, 3);
        // DSH commits milliseconds; keep every record after the epoch second.
        set_record_times(&projection, |seq| epoch * 1000 + 1000 + seq as i64)?;
        let report = rt.reconcile(&projection)?;
        assert_eq!(report.queued, Some(0), "reconcile is not a capture source");

        // The harness only flushes later, so make turn 3 end after enablement
        // (milliseconds, as DSH commits them) and re-read the projection.
        let epoch = rt.state.project_epoch(&rt.id)?;
        set_turn_times(&projection, 3, |seq| (epoch + 1) * 1000 + seq as i64)?;
        let report = rt.enqueue("session-x", 3)?;
        assert_eq!(report.state, Some("queued"), "the live hint queues its run");

        let report = rt.capture(&projection, None)?;
        assert_eq!(report.state, Some("ready"), "{report:?}");
        let digest = report.evidence.expect("evidence digest");
        let evidence = coding_memory::evidence::load(&rt.root, &digest)?;
        assert_eq!(evidence.outcome, "completed");
        assert_eq!(evidence.project, rt.id);
        assert_eq!(evidence.run.turn, "3");
        assert_eq!(
            evidence.items.len(),
            1,
            "only the human message is evidence"
        );

        // Replaying the same session never recaptures a finished turn.
        let report = rt.capture(&projection, None)?;
        assert_eq!(report.reason.as_deref(), Some("no_ready_run"));
        Ok(())
    }

    #[test]
    fn a_run_no_live_process_saw_end_is_never_captured() -> anyhow::Result<()> {
        // Reconciliation can queue a run whose end this process never observed.
        // Publishing it from a session read would capture the wrong run, so
        // capture refuses without a hint.
        let mut rt = Runtime::new()?;
        let projection = rt.path("session.json");
        // Register while the session has one completed turn: that turn is the
        // enablement baseline. The turn to capture arrives afterwards.
        session(&projection, &rt.cwd, 1);
        rt.register(&projection)?;
        // Second-resolution boundary: the new turn starts in a later second.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let epoch = ingestion::now();
        session(&projection, &rt.cwd, 2);
        // DSH commits milliseconds; keep the turn after the epoch second.
        set_record_times(&projection, |seq| epoch * 1000 + 1000 + seq as i64)?;

        // Exactly what reconciliation produces: a queued run with no hint.
        let key = rt.key("session-x", 2);
        rt.state.enqueue(&key, &rt.id, &rt.cwd, 0)?;
        let report = rt.capture(&projection, None)?;
        assert_eq!(report.reason.as_deref(), Some("no_capture_hint"));
        assert!(report.evidence.is_none(), "no evidence is published");
        assert_eq!(report.run.as_ref().map(|r| r.turn.as_str()), Some("2"));
        assert_eq!(
            rt.state.status()?[0].state,
            "held",
            "the run stays inspectable rather than silently skipped"
        );
        Ok(())
    }

    #[test]
    fn a_hint_recorded_before_registration_resolves_after_it() -> anyhow::Result<()> {
        // A live run can end before its project is registered. The hint names
        // the exact run that ended, so reconcile queues it once registration
        // enables the project instead of dropping it.
        let mut rt = Runtime::new()?;
        let projection = rt.path("session.json");
        session(&projection, &rt.cwd, 1);
        let report = rt.enqueue("session-x", 1)?;
        assert_eq!(
            report.state,
            Some("deferred"),
            "the hint is kept, not dropped"
        );
        assert_eq!(rt.state.pending_hints(&rt.store)?.len(), 1);

        // The harness only flushes on teardown, so the run ends in an earlier
        // second than the registration that enables the project.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        rt.register(&projection)?;
        let report = rt.reconcile(&projection)?;
        assert_eq!(
            report.queued,
            Some(0),
            "a turn ending before enablement is excluded"
        );
        assert!(
            rt.state.pending_hints(&rt.store)?.is_empty(),
            "the hint is resolved"
        );

        // A run that ends after registration resolves into its exact run.
        let epoch = ingestion::now();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        session(&projection, &rt.cwd, 2);
        // DSH commits milliseconds; keep the turn after the epoch second.
        set_record_times(&projection, |seq| epoch * 1000 + 1000 + seq as i64)?;
        rt.enqueue("session-x", 2)?;
        assert_eq!(
            rt.state
                .enqueue_hinted(&rt.key("session-x", 2), &rt.id, &rt.cwd, 0)?,
            Enqueue::Duplicate,
            "the hinted run is already queued"
        );
        Ok(())
    }

    #[test]
    fn capture_claims_the_session_its_projection_describes() -> anyhow::Result<()> {
        // Two sessions of one store queued at once: a capture must publish the
        // projection's own session, never the store's oldest run.
        let mut rt = Runtime::new()?;
        let old = rt.path("old.json");
        projection_for(&old, &rt.cwd, "session-old", 1);
        rt.register(&old)?;
        let later = (ingestion::now() + 5) * 1000;
        let a = rt.path("a.json");
        let b = rt.path("b.json");
        projection_for(&a, &rt.cwd, "session-a", later);
        projection_for(&b, &rt.cwd, "session-b", later);
        rt.enqueue("session-a", 1)?;
        rt.enqueue("session-b", 1)?;

        // Capture session-b first; session-a's older run must stay queued.
        let report = rt.capture(&b, None)?;
        assert_eq!(report.state, Some("ready"), "{report:?}");
        assert_eq!(
            report.run.as_ref().map(|r| r.thread.as_str()),
            Some("session-b")
        );
        let digest = report.evidence.expect("digest");
        let evidence = coding_memory::evidence::load(&rt.root, &digest)?;
        assert_eq!(
            evidence.run.thread, "session-b",
            "evidence belongs to the projection"
        );
        let status = rt.state.status()?;
        assert_eq!(status.len(), 2);
        assert_eq!(
            status
                .iter()
                .find(|s| s.key.thread == "session-a")
                .map(|s| s.state.as_str()),
            Some("queued"),
            "the other session's run is untouched"
        );
        assert_eq!(evidence.project, rt.id);
        Ok(())
    }

    #[test]
    fn reconciling_one_session_leaves_another_sessions_hint_pending() -> anyhow::Result<()> {
        let mut rt = Runtime::new()?;
        let a = rt.path("a.json");
        let b = rt.path("b.json");
        projection_for(&a, &rt.cwd, "session-a", 1);
        projection_for(&b, &rt.cwd, "session-b", 1);
        // Both hints arrive before registration.
        rt.enqueue("session-a", 1)?;
        rt.enqueue("session-b", 1)?;
        rt.register(&a)?;
        rt.reconcile(&a)?;
        let pending = rt.state.pending_hints(&rt.store)?;
        assert_eq!(pending.len(), 1, "session-b's hint is not consumed");
        assert_eq!(pending[0].0.thread, "session-b");
        Ok(())
    }

    #[test]
    fn registering_twice_never_baselines_later_turns() -> anyhow::Result<()> {
        let mut rt = Runtime::new()?;
        let path = rt.path("session.json");
        session(&path, &rt.cwd, 1);
        rt.register(&path)?;
        let epoch = rt.state.project_epoch(&rt.id)?;
        let later = (epoch + 5) * 1000;
        projection_for(&path, &rt.cwd, "session-x", later);
        // Registering again is still idempotent even though the projection now
        // shows a later turn.
        rt.register(&path)?;
        rt.enqueue("session-x", 2)?;
        // Re-registering must not baseline the queued post-enablement run.
        let report = rt.register(&path)?;
        assert_eq!(report.reason.as_deref(), Some("already_registered"));
        assert_eq!(
            rt.state
                .status()?
                .iter()
                .find(|s| s.key.thread == "session-x" && s.key.turn == "2")
                .map(|s| s.state.as_str()),
            Some("queued"),
            "re-registering must not baseline the queued post-enablement run"
        );
        Ok(())
    }

    #[test]
    fn re_registering_does_not_read_the_projection() -> anyhow::Result<()> {
        // Registration is a one-time boundary. A registered project must report
        // `already_registered` even when the bootstrap snapshot is already gone,
        // because no baseline is applied and no state changes.
        let mut rt = Runtime::new()?;
        let path = rt.path("session.json");
        session(&path, &rt.cwd, 1);
        rt.register(&path)?;
        std::fs::remove_file(&path)?;
        let report = rt.register(&path)?;
        assert_eq!(report.reason.as_deref(), Some("already_registered"));
        assert_eq!(report.baseline, Some(0));
        Ok(())
    }

    #[test]
    fn an_unresolvable_project_holds_the_claimed_run() -> anyhow::Result<()> {
        // A recorded cwd that no longer resolves must be held with a reason,
        // not re-queued and re-selected by every later capture attempt.
        let mut rt = Runtime::new()?;
        let path = rt.path("session.json");
        session(&path, &rt.cwd, 1);
        rt.register(&path)?;
        let later = (ingestion::now() + 5) * 1000;
        let projection = rt.path("gone.json");
        projection_for(&projection, &rt.cwd, "session-z", later);
        rt.enqueue("session-z", 1)?;
        // The project directory disappears after the run was queued.
        std::fs::remove_dir_all(&rt.repo)?;
        let report = rt.capture(&projection, None)?;
        assert_eq!(report.reason.as_deref(), Some("project_mismatch"));
        assert_eq!(
            rt.status_of("session-z", 1)?.map(|(state, _)| state),
            Some("held".into()),
            "the run is held instead of staying queued"
        );
        Ok(())
    }

    #[test]
    fn a_hint_is_persisted_with_its_run_atomically() -> anyhow::Result<()> {
        let mut rt = Runtime::new()?;
        let path = rt.path("session.json");
        session(&path, &rt.cwd, 1);
        rt.register(&path)?;
        let key = rt.key("session-y", 1);
        rt.state.enqueue_hinted(&key, &rt.id, &rt.cwd, 0)?;
        assert!(
            rt.state.hinted(&key)?,
            "a queued run always carries its hint"
        );
        Ok(())
    }

    #[test]
    fn a_historical_hint_reports_before_enablement_not_a_fork() -> anyhow::Result<()> {
        // A hinted turn that predates registration must be held with a reason
        // naming the boundary, so operators are not told it was excluded as a
        // fork or a nonhuman run.
        let mut rt = Runtime::new()?;
        let path = rt.path("session.json");
        session(&path, &rt.cwd, 1);
        rt.register(&path)?;
        let epoch = rt.state.project_epoch(&rt.id)?;
        // A second projected turn ending in an earlier second than registration.
        projection_for(&path, &rt.cwd, "session-h", epoch.saturating_sub(60) * 1000);
        let report = rt.enqueue("session-h", 1)?;
        assert!(
            matches!(report.state, Some("queued") | Some("held")),
            "{report:?}"
        );
        let report = rt.capture(&path, None)?;
        assert_eq!(
            report.reason.as_deref(),
            Some("before_enablement"),
            "{report:?}"
        );
        assert_eq!(
            rt.state.status()?[0].reason.as_deref(),
            Some("before_enablement"),
            "status names the enablement boundary, not a fork"
        );
        Ok(())
    }

    #[test]
    fn capture_turn_never_leases_another_turn_of_the_same_session() -> anyhow::Result<()> {
        // A capture pinned to turn 2 must not lease turn 1's queued run, which
        // would hold it `capturing` for the lease hour without being usable.
        let mut rt = Runtime::new()?;
        // Register against a different session, so the baseline it records
        // names that session only; the two-turn projection below is new work.
        let baseline = rt.path("baseline.json");
        session_as(&baseline, &rt.cwd, 1, "session-baseline");
        rt.register(&baseline)?;

        let later = (ingestion::now() + 5) * 1000;
        let projection = rt.path("later.json");
        two_turn_projection(&projection, &rt.cwd, "session-x", later);
        rt.enqueue("session-x", 1)?;
        rt.enqueue("session-x", 2)?;
        let report = rt.capture(&projection, Some(2))?;
        assert_eq!(report.run.as_ref().map(|r| r.turn.as_str()), Some("2"));
        let turns: Vec<_> = rt
            .state
            .status()?
            .iter()
            .map(|s| (s.key.turn.clone(), s.state.clone(), s.reason.clone()))
            .collect();
        assert_eq!(
            turns,
            vec![
                ("1".into(), "queued".into(), None),
                ("2".into(), "ready".into(), None)
            ],
            "a pinned capture leaves the other turn queued instead of leasing it"
        );
        Ok(())
    }

    #[test]
    fn a_persistently_pending_projection_stops_retrying() -> anyhow::Result<()> {
        // A projection that never records the turn's end must become inspectable
        // after the same bounded attempts the Codex reader uses.
        let mut rt = Runtime::new()?;
        let path = rt.path("open.json");
        // One turn with a start, a human message, and no turn/end.
        write_document(
            &path,
            &document(
                &rt.cwd,
                "session-open",
                1,
                vec![
                    json!({"seq":0,"type":"turn/start","time":1,"data":{"turn":1}}),
                    json!({"seq":1,"type":"user/message","time":2,"surfaceOp":"append","data":{
                        "id":"m1","role":"user","content":[{"type":"text","text":"work"}],"source":{"kind":"user"}}}),
                ],
            ),
        );
        let registration = rt.path("reg.json");
        session(&registration, &rt.cwd, 1);
        rt.register(&registration)?;
        let later = (ingestion::now() + 5) * 1000;
        set_record_times(&path, |_| later)?;
        rt.enqueue("session-open", 1)?;

        // Deferrals make the run unavailable for the retry delay; capture again
        // as a worker cycle would, until the attempt budget is exhausted.
        let mut last = None;
        for _ in 0..70 {
            if rt
                .status_of("session-open", 1)?
                .is_some_and(|(state, reason)| {
                    state == "queued" && reason.as_deref() == Some("history_pending")
                })
            {
                make_available(&rt.root, "session-open")?;
            }
            let report = rt.capture(&path, None)?;
            let Some(reason) = report.reason else {
                break;
            };
            last = Some(reason);
            if last.as_deref() == Some("history_readiness_exhausted") {
                break;
            }
        }
        assert_eq!(
            last.as_deref(),
            Some("history_readiness_exhausted"),
            "retries are bounded"
        );
        Ok(())
    }

    #[test]
    fn a_missing_signal_project_is_rejected() -> anyhow::Result<()> {
        let mut rt = Runtime::new()?;
        let signal = json!({"kind":"turn-end","session":"s","turn":1,"cwd":"/nonexistent/project"});
        assert!(rt.enqueue_value(&signal).is_err());
        Ok(())
    }

    #[test]
    fn exclusion_and_ownership_hold_runs_at_enqueue_time() -> anyhow::Result<()> {
        let mut rt = Runtime::new()?;
        let projection = rt.path("session.json");
        session(&projection, &rt.cwd, 1);
        rt.register(&projection)?;
        let owned = rt.own_store()?;
        assert_eq!(owned.action, "own-store");
        let report = rt.enqueue("session-x", 1)?;
        assert_eq!(
            report.state,
            Some("held"),
            "engine-owned stores never queue"
        );
        assert_eq!(
            rt.state
                .enqueue(&rt.key("session-x", 1), &rt.id, &rt.cwd, 0)?,
            Enqueue::Excluded
        );
        Ok(())
    }

    #[test]
    fn registration_keeps_a_turn_that_ends_after_the_boundary() -> anyhow::Result<()> {
        // The baseline is "ended at or before enablement", not "present during
        // the scan". Excluding the latter would discard a valid hinted run.
        let mut rt = Runtime::new()?;
        let time = (ingestion::now() + 3600) * 1000;
        let projection = rt.path("session.json");
        write_document(
            &projection,
            &document(
                &rt.cwd,
                "session-x",
                time,
                vec![
                    json!({"seq":0,"type":"turn/start","time":time,"data":{"turn":1}}),
                    json!({"seq":1,"type":"user/message","time":time+1,"data":{
                        "id":"m1","role":"user","content":[{"type":"text","text":"work"}],
                        "source":{"kind":"user"}}}),
                    json!({"seq":2,"type":"turn/end","time":time+2,"data":{"turn":1,"reason":{"kind":"completed"}}}),
                ],
            ),
        );
        let hinted = rt.enqueue("session-x", 1)?;
        assert_eq!(hinted.state, Some("deferred"));
        let registered = rt.register(&projection)?;
        assert_eq!(
            registered.baseline,
            Some(0),
            "a turn ending after the boundary is not the baseline"
        );
        let reconciled = rt.reconcile(&projection)?;
        assert_eq!(
            reconciled.queued,
            Some(1),
            "the post-enablement hinted turn is queued"
        );
        assert!(rt.state.pending_hints(&rt.store)?.is_empty());
        Ok(())
    }

    #[test]
    fn capture_compares_the_exact_registration_instant() -> anyhow::Result<()> {
        // The whole-second epoch would admit a turn that ended earlier in the
        // same second the project was enabled.
        let mut rt = Runtime::new()?;
        let baseline = rt.path("baseline.json");
        session(&baseline, &rt.cwd, 0);
        rt.register(&baseline)?;
        let enabled_ms = rt
            .state
            .project_epoch_ms(&rt.id)?
            .expect("registration records the exact instant");

        let before = rt.path("before.json");
        timed_projection(&before, &rt.cwd, 1, enabled_ms - 1000, enabled_ms - 1);
        let after = rt.path("after.json");
        timed_projection(&after, &rt.cwd, 2, enabled_ms - 1000, enabled_ms + 1);
        rt.enqueue("session-x", 1)?;
        rt.enqueue("session-x", 2)?;

        let held = rt.capture(&before, Some(1))?;
        assert_eq!(held.state, Some("held"));
        assert_eq!(held.reason.as_deref(), Some("before_enablement"));
        let ready = rt.capture(&after, Some(2))?;
        assert_eq!(
            ready.state,
            Some("ready"),
            "a turn ending after the exact instant is captured"
        );
        Ok(())
    }

    #[test]
    fn an_oversized_turn_is_held_without_publishing_partial_evidence() -> anyhow::Result<()> {
        let mut rt = Runtime::new()?;
        let baseline = rt.path("baseline.json");
        session(&baseline, &rt.cwd, 0);
        rt.register(&baseline)?;
        rt.enqueue("session-x", 1)?;
        // The evidence item alone exceeds the engine's 8 MiB bundle bound while
        // the projection stays inside the 16 MiB artifact bound.
        let huge = "x".repeat(8 * 1024 * 1024 + 1024);
        let time = (ingestion::now() + 5) * 1000;
        let projection = rt.path("huge.json");
        write_document(
            &projection,
            &document(
                &rt.cwd,
                "session-x",
                time,
                vec![
                    json!({"seq":0,"type":"turn/start","time":time,"data":{"turn":1}}),
                    json!({"seq":1,"type":"user/message","time":time+1,"data":{
                        "id":"m1","role":"user","content":[{"type":"text","text":"work"}],
                        "source":{"kind":"user"}}}),
                    json!({"seq":2,"type":"tool/call","time":time+2,"data":{
                        "turn":1,"step":1,"callId":"c1","name":"read","arguments":"{}"}}),
                    json!({"seq":3,"type":"tool/result","time":time+3,"data":{
                        "turn":1,"step":1,"message":{"id":"t1","role":"tool","toolCallId":"c1",
                        "content":[{"type":"text","text":huge}]}}}),
                    json!({"seq":4,"type":"turn/end","time":time+4,"data":{"turn":1,"reason":{"kind":"completed"}}}),
                ],
            ),
        );
        let report = rt.capture(&projection, Some(1))?;
        assert_eq!(report.state, Some("held"));
        assert_eq!(report.reason.as_deref(), Some("history_over_limit"));
        assert!(
            report.evidence.is_none(),
            "no partial evidence is published"
        );
        Ok(())
    }

    #[test]
    fn an_oversized_turn_can_be_held_without_a_projection() -> anyhow::Result<()> {
        // The bridge cannot publish a turn over its transport bound, so it asks
        // the engine to hold the already-queued run instead of leaving it queued.
        let mut rt = Runtime::new()?;
        let baseline = rt.path("baseline.json");
        session(&baseline, &rt.cwd, 0);
        rt.register(&baseline)?;
        rt.enqueue("session-x", 1)?;
        let report = rt.hold_over_limit("session-x", 1)?;
        assert_eq!(report.state, Some("held"));
        assert_eq!(report.reason.as_deref(), Some("history_over_limit"));
        let status = rt.state.status()?;
        assert_eq!(status.len(), 1);
        assert_eq!(status[0].state, "held");
        assert_eq!(
            status[0].reason.as_deref(),
            Some("history_over_limit"),
            "the run leaves the queue as an inspectable hold"
        );
        Ok(())
    }

    #[test]
    fn a_schema_invalid_projection_holds_its_claimed_run() -> anyhow::Result<()> {
        // The run must not stay queued (or be leased for an hour) merely because
        // the bridge document declares a schema this engine cannot read.
        let mut rt = Runtime::new()?;
        let baseline = rt.path("baseline.json");
        session(&baseline, &rt.cwd, 0);
        rt.register(&baseline)?;
        let queued = rt.enqueue("session-x", 2)?;
        assert_eq!(queued.state, Some("queued"));

        let projection = rt.path("future.json");
        write_document(
            &projection,
            &json!({"schema":2,"header":{
                "type":"session","version":4,"id":"session-x","createdAt":1,"cwd":rt.cwd,"isSeeded":false},
                "records":[]}),
        );
        let report = rt.capture(&projection, Some(2))?;
        assert_eq!(report.state, Some("held"));
        assert_eq!(
            report.reason.as_deref(),
            Some("unsupported_projection_schema")
        );
        let status = rt.state.status()?;
        assert_eq!(status.len(), 1);
        assert_eq!(status[0].state, "held");
        assert_eq!(
            status[0].reason.as_deref(),
            Some("unsupported_projection_schema"),
            "the claimed run reaches the documented held outcome"
        );
        Ok(())
    }

    #[test]
    fn registration_rejects_an_unsupported_projection_schema() -> anyhow::Result<()> {
        // Registration fixes the enablement boundary permanently, so it must not
        // baseline a document whose bridge schema is unsupported.
        let mut rt = Runtime::new()?;
        let projection = rt.path("session.json");
        session(&projection, &rt.cwd, 1);
        let mut document: Value = serde_json::from_slice(&std::fs::read(&projection)?)?;
        document["schema"] = json!(2);
        std::fs::write(&projection, serde_json::to_vec(&document)?)?;
        assert!(
            rt.register(&projection).is_err(),
            "an unsupported projection schema is refused"
        );
        assert!(
            rt.state.project_epoch(&rt.id).is_err(),
            "the project must remain unregistered"
        );

        document["schema"] = json!(1);
        std::fs::write(&projection, serde_json::to_vec(&document)?)?;
        let report = rt.register(&projection)?;
        assert_eq!(
            report.baseline,
            Some(1),
            "a supported projection registers and baselines its turn"
        );
        Ok(())
    }

    #[test]
    fn registration_rejects_an_unsupported_logical_version() -> anyhow::Result<()> {
        // A permanent boundary must not be recorded for a generation whose
        // turns capture would reject on every attempt.
        let mut rt = Runtime::new()?;
        let projection = rt.path("session.json");
        session(&projection, &rt.cwd, 1);
        let mut document: Value = serde_json::from_slice(&std::fs::read(&projection)?)?;
        document["header"]["version"] = json!(5);
        std::fs::write(&projection, serde_json::to_vec(&document)?)?;
        assert!(
            rt.register(&projection).is_err(),
            "a newer logical format is refused before enablement"
        );
        assert!(
            rt.state.project_epoch(&rt.id).is_err(),
            "the project stays unregistered"
        );
        Ok(())
    }

    #[test]
    fn reconciling_one_projection_leaves_another_turns_hint_pending() -> anyhow::Result<()> {
        // The bridge writes one projection per turn. Reconciling turn 3 must not
        // permanently exclude turn 4 just because this file does not contain it.
        let mut rt = Runtime::new()?;
        let baseline = rt.path("baseline.json");
        session(&baseline, &rt.cwd, 0);
        rt.register(&baseline)?;
        let hint3 = rt.key("session-x", 3);
        let hint4 = rt.key("session-x", 4);
        rt.state.save_pending_hint(&hint3, &rt.cwd, 0)?;
        rt.state.save_pending_hint(&hint4, &rt.cwd, 0)?;

        let time = (ingestion::now() + 5) * 1000;
        let projection = rt.path("turn3.json");
        write_document(
            &projection,
            &document(
                &rt.cwd,
                "session-x",
                1,
                vec![
                    json!({"seq":0,"type":"turn/start","time":time,"data":{"turn":3}}),
                    json!({"seq":1,"type":"user/message","time":time+1,"data":{
                        "turn":3,"step":1,"id":"m3","role":"user",
                        "content":[{"type":"text","text":"work"}],"source":{"kind":"user"}}}),
                    json!({"seq":2,"type":"turn/end","time":time+2,"data":{"turn":3,"reason":{"kind":"completed"}}}),
                ],
            ),
        );
        let report = rt.reconcile(&projection)?;
        assert_eq!(report.queued, Some(1), "the turn in this file is queued");
        let pending = rt.state.pending_hints(&rt.store)?;
        assert_eq!(pending.len(), 1, "the absent turn's hint is retained");
        assert_eq!(pending[0].0.turn, "4");
        let status = rt.state.status()?;
        assert!(
            status
                .iter()
                .any(|run| run.key.turn == "3" && run.state == "queued"),
            "turn 3 is queued"
        );
        assert!(
            status.iter().all(|run| run.key.turn != "4"),
            "turn 4 was not excluded as a historical run"
        );
        Ok(())
    }

    #[test]
    fn reconciling_a_nonhuman_turn_resolves_it_as_excluded() -> anyhow::Result<()> {
        // A completed turn with no direct human message is represented in the
        // projection. It must reach the terminal exclusion, not stay pending as
        // though the turn were missing from an older snapshot.
        let mut rt = Runtime::new()?;
        let baseline = rt.path("baseline.json");
        session(&baseline, &rt.cwd, 0);
        rt.register(&baseline)?;
        let hint = rt.key("session-x", 1);
        rt.state.save_pending_hint(&hint, &rt.cwd, 0)?;
        let time = (ingestion::now() + 5) * 1000;
        let projection = rt.path("nonhuman.json");
        write_document(
            &projection,
            &document(
                &rt.cwd,
                "session-x",
                time,
                vec![
                    json!({"seq":0,"type":"turn/start","time":time,"data":{"turn":1}}),
                    json!({"seq":1,"type":"user/message","time":time+1,"data":{
                        "id":"ctx","role":"user","content":[{"type":"text","text":"injected"}],
                        "source":{"kind":"time-context"}}}),
                    json!({"seq":2,"type":"turn/end","time":time+2,"data":{"turn":1,"reason":{"kind":"completed"}}}),
                ],
            ),
        );
        let report = rt.reconcile(&projection)?;
        assert_eq!(report.queued, Some(0), "a nonhuman turn is not queued");
        assert!(rt.state.pending_hints(&rt.store)?.is_empty());
        assert_eq!(
            rt.state.enqueue(&hint, &rt.id, &rt.cwd, 0)?,
            Enqueue::Excluded,
            "the nonhuman turn is terminally excluded"
        );
        Ok(())
    }

    #[test]
    fn dsh_store_identity_is_namespaced_from_codex() -> anyhow::Result<()> {
        let dir = tempdir()?;
        assert_ne!(
            ingestion::store_id(dir.path())?,
            ingestion::dsh_store_id(dir.path())?,
            "a shared directory must not collapse both runtimes into one store"
        );
        assert_eq!(
            ingestion::dsh_store_id(dir.path())?,
            ingestion::dsh_store_id(dir.path())?,
            "the runtime-qualified identity is stable"
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
