//! Opt-in cleanup of old no-value evidence; every other source is retained.
use crate::{domain::hash, evidence, extraction::Candidate, search::Search, state::State};
use anyhow::{Result, ensure};
use rusqlite::params;
use std::path::Path;
pub fn cleanup(
    state: &mut State,
    root: &Path,
    vault: &Path,
    now: i64,
    days: u64,
    apply: bool,
) -> Result<Vec<String>> {
    ensure!(
        (30..=36500).contains(&days),
        "Retention must be at least thirty days"
    );
    let search = Search::new(root, vault)?;
    let report = search.rebuild()?;
    ensure!(
        report.invalid.is_empty(),
        "Invalid notes prevent evidence cleanup"
    );
    let mut retained = search.retained_evidence()?;
    let tx = state
        .db
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    ensure!(!tx.query_row("SELECT EXISTS(SELECT 1 FROM consolidation_jobs WHERE state IN ('generating','applying','conflict'))",[],|r|r.get::<_,bool>(0))?,"Unresolved publication prevents evidence cleanup");
    let candidates = {
        let mut q = tx.prepare("SELECT candidate FROM candidates")?;
        q.query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    for json in candidates {
        let c: Candidate = serde_json::from_str(&json)?;
        for r in c.evidence {
            retained.insert(r.bundle);
        }
    }
    let cutoff = now.saturating_sub(i64::try_from(days * 86400)?);
    let digests = {
        let mut q=tx.prepare("SELECT evidence FROM extraction_jobs WHERE state='succeeded_empty' AND finished_at<=?1")?;
        q.query_map([cutoff], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    let mut eligible = Vec::new();
    for digest in digests {
        if retained.contains(&digest) {
            continue;
        }
        ensure!(
            digest.len() == 64
                && digest
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "Invalid evidence digest"
        );
        let path = root.join("evidence").join(format!("{digest}.json"));
        let meta = match std::fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e.into()),
        };
        ensure!(
            meta.is_file() && !meta.file_type().is_symlink(),
            "Nonregular evidence prevents cleanup"
        );
        ensure!(
            hash(&evidence::read_bounded(&path, evidence::MAX_SOURCE_BYTES)?) == digest,
            "Corrupt no-value evidence"
        );
        eligible.push(digest.clone());
        if apply {
            std::fs::remove_file(path)?;
            std::fs::File::open(root.join("evidence"))?.sync_all()?;
            tx.execute(
                "INSERT OR REPLACE INTO evidence_cleanup VALUES(?1,?2)",
                params![digest, now],
            )?;
        }
    }
    tx.commit()?;
    Ok(eligible)
}
