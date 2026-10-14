//! Atomic per-file application with durable SQLite barriers and restart recognition.
use crate::{
    consolidation_state::Change, domain::hash, evidence, notes, search::Search, state::State,
};
use anyhow::{Result, ensure};
use rusqlite::params;
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Component, Path},
};
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Point {
    BeforeReplace,
    AfterReplace,
    BeforeFinalize,
}
fn target(vault: &Path, relative: &str) -> Result<std::path::PathBuf> {
    let p = Path::new(relative);
    ensure!(
        !p.as_os_str().is_empty()
            && p.components().all(|c| matches!(c, Component::Normal(_)))
            && p.extension().is_some_and(|e| e == "md"),
        "Invalid change path"
    );
    let result = vault.join(p);
    let parent = result
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Missing parent"))?;
    let mut current = vault.to_path_buf();
    for c in p.parent().unwrap_or(Path::new("")).components() {
        current.push(c);
        ensure!(
            !fs::symlink_metadata(&current)?.file_type().is_symlink(),
            "Symlinked change directory"
        );
    }
    ensure!(
        parent.canonicalize()?.starts_with(vault),
        "Change escapes vault"
    );
    if let Ok(m) = fs::symlink_metadata(&result) {
        ensure!(
            m.is_file() && !m.file_type().is_symlink(),
            "Nonregular change target"
        );
    }
    Ok(result)
}
fn revision(path: &Path) -> Result<Option<String>> {
    match evidence::read_bounded(path, notes::MAX_NOTE_BYTES) {
        Ok(b) => Ok(Some(hash(&b))),
        Err(e)
            if e.downcast_ref::<std::io::Error>()
                .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
        {
            Ok(None)
        }
        Err(e) => Err(e),
    }
}
fn replace(
    path: &Path,
    change: &Change,
    batch: i64,
    checkpoint: &mut impl FnMut(Point) -> Result<()>,
) -> Result<()> {
    let parent = path.parent().unwrap();
    let temp = parent.join(format!(".memory-{batch}-{}.tmp", change.note_id));
    // An interrupted temporary write is never authoritative; overwrite only our own regular file.
    if let Ok(m) = fs::symlink_metadata(&temp) {
        ensure!(
            m.is_file() && !m.file_type().is_symlink(),
            "Invalid application temporary file"
        );
        fs::remove_file(&temp)?;
    }
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temp)?;
    file.write_all(change.content.as_bytes())?;
    file.sync_all()?;
    checkpoint(Point::BeforeReplace)?;
    fs::rename(&temp, path)?;
    File::open(parent)?.sync_all()?;
    checkpoint(Point::AfterReplace)?;
    Ok(())
}
pub fn apply(
    state: &mut State,
    root: &Path,
    vault: &Path,
    batch: i64,
    mut checkpoint: impl FnMut(Point) -> Result<()>,
) -> Result<bool> {
    let vault = vault.canonicalize()?;
    ensure!(
        state.batch_vault(batch)?.as_deref() == vault.to_str(),
        "Recovery vault does not match recorded vault"
    );
    let phase: String = state.db.query_row(
        "SELECT state FROM consolidation_jobs WHERE id=?1",
        [batch],
        |r| r.get(0),
    )?;
    if phase == "applied" {
        return Ok(true);
    }
    ensure!(
        ["applying", "conflict"].contains(&phase.as_str()),
        "Batch has no reviewed application"
    );
    let changes = {
        let mut q=state.db.prepare("SELECT note_id,path,before_hash,after_hash,content FROM vault_changes WHERE batch=?1 ORDER BY rowid")?;
        q.query_map([batch], |r| {
            Ok(Change {
                note_id: r.get(0)?,
                path: r.get(1)?,
                before: r.get(2)?,
                after: r.get(3)?,
                content: r.get(4)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?
    };
    for change in &changes {
        let tx = state
            .db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let path = target(&vault, &change.path)?;
        let current = revision(&path)?;
        if current.as_deref() != Some(&change.after) && current != change.before {
            tx.execute("UPDATE consolidation_jobs SET state='conflict',reason='revision_conflict' WHERE id=?1",[batch])?;
            tx.commit()?;
            return Ok(false);
        }
        if current.as_deref() != Some(&change.after) {
            replace(&path, change, batch, &mut checkpoint)?;
        } else {
            File::open(&path)?.sync_all()?;
            File::open(path.parent().unwrap())?.sync_all()?;
        }
        tx.execute(
            "UPDATE vault_changes SET applied=1 WHERE batch=?1 AND note_id=?2",
            params![batch, change.note_id],
        )?;
        tx.commit()?;
    }
    // Index refresh may fail; barriers remain until both disk and index recovery complete.
    let report = Search::new(root, &vault)?.rebuild()?;
    ensure!(
        report
            .invalid
            .iter()
            .all(|n| !changes.iter().any(|c| c.path == n.path)),
        "Applied note unavailable in index"
    );
    checkpoint(Point::BeforeFinalize)?;
    let tx = state
        .db
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    for change in &changes {
        if revision(&target(&vault, &change.path)?)?.as_deref() != Some(&change.after) {
            tx.execute("UPDATE consolidation_jobs SET state='conflict',reason='revision_conflict' WHERE id=?1",[batch])?;
            tx.commit()?;
            return Ok(false);
        }
    }
    let consumed: String = tx.query_row(
        "SELECT consumed FROM consolidation_jobs WHERE id=?1",
        [batch],
        |r| r.get(0),
    )?;
    let consumed: Vec<String> = serde_json::from_str(&consumed)?;
    tx.execute("UPDATE candidates SET state='held' WHERE id IN (SELECT candidate FROM batch_candidates WHERE batch=?1)",[batch])?;
    for id in consumed {
        ensure!(tx.execute("UPDATE candidates SET state='consumed' WHERE id=?1 AND id IN (SELECT candidate FROM batch_candidates WHERE batch=?2)",params![id,batch])?==1,"Consumed candidate outside batch");
    }
    for change in &changes {
        tx.execute(
            "DELETE FROM pending_notes WHERE note_id=?1 AND reason=?2",
            params![change.note_id, format!("batch-{batch}")],
        )?;
    }
    tx.execute(
        "UPDATE consolidation_jobs SET state='applied',reason=NULL WHERE id=?1",
        [batch],
    )?;
    tx.commit()?;
    Ok(true)
}
