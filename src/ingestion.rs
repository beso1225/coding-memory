use crate::{
    codex::{self, Capture, Client},
    domain::{RunKey, hash},
    evidence, project,
    state::{Enqueue, State},
};
use anyhow::{Result, ensure};
use serde_json::{Value, json};
use std::{collections::HashMap, path::Path};

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
        Ok(Capture::Ready(bundle)) => {
            let epoch = state.project_epoch(&claim.project)?;
            if bundle.started_at <= epoch {
                state.hold(&claim, "before_enablement_or_unknown_start", now())?;
                return Ok(true);
            }
            let digest = evidence::save(root, &bundle)?;
            state.finish(&claim, &bundle.outcome, Some(&digest), now())?;
        }
        Ok(Capture::Pending) if claim.attempts >= 60 => {
            state.hold(&claim, "history_readiness_exhausted", now())?
        }
        Ok(Capture::Pending) => state.defer(&claim, "history_pending", now(), 10)?,
        Ok(Capture::Missing) => state.hold(&claim, "history_missing", now())?,
        Ok(Capture::Unsupported) => state.hold(&claim, "unsupported_format", now())?,
        Ok(Capture::OverLimit) => state.hold(&claim, "history_over_limit", now())?,
        Ok(Capture::Excluded) => state.hold(&claim, "excluded_inherited_or_nonhuman", now())?,
        Ok(Capture::WrongProject) => state.hold(&claim, "project_mismatch", now())?,
        Err(error) => state.hold(
            &claim,
            match error.downcast_ref::<codex::ReadFailure>() {
                Some(codex::ReadFailure::Missing) => "history_missing",
                Some(codex::ReadFailure::Unsupported) => "unsupported_format",
                Some(codex::ReadFailure::OverLimit) => "history_over_limit",
                None => "reader_failure",
            },
            now(),
        )?,
    }
    Ok(true)
}
