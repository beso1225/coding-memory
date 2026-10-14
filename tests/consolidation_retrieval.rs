use anyhow::Result;
use coding_memory::{
    consolidation_retrieval::neighbors,
    extraction::{Candidate, CandidateKind},
    notes::{self, MemoryScope, Verification},
    search::{Scope, Search},
    state::State,
};
use std::fs;
use tempfile::tempdir;
fn candidate(title: &str, claim: &str) -> Candidate {
    Candidate {
        kind: CandidateKind::Constraint,
        title: title.into(),
        claim: claim.into(),
        scope: MemoryScope::Project {
            project_id: "alpha".into(),
        },
        conditions: vec!["SQLite WAL".into()],
        verification: Verification::Observed,
        evidence: vec![],
    }
}
fn write(
    vault: &std::path::Path,
    id: &str,
    title: &str,
    body: &str,
    scope: MemoryScope,
) -> Result<()> {
    let mut n = notes::parse(include_str!("fixtures/vault/queue.md"))?;
    n.metadata.id = id.into();
    n.metadata.title = title.into();
    n.metadata.summary = body.into();
    n.metadata.scope = scope;
    n.body = body.into();
    fs::write(vault.join(format!("{id}.md")), notes::render(&n)?)?;
    Ok(())
}
#[test]
fn paraphrases_recover_duplicates_and_related_notes_without_foreign_or_pending_hits() -> Result<()>
{
    let d = tempdir()?;
    let root = d.path().join("state");
    let vault = d.path().join("vault");
    fs::create_dir(&root)?;
    fs::create_dir(&vault)?;
    let mut state = State::open(&root.join("state.sqlite"))?;
    write(
        &vault,
        "links",
        "Related Wikilinks use supplied paths and aliases",
        "Wikilinks resolve validated paths to stable IDs.",
        MemoryScope::Project {
            project_id: "alpha".into(),
        },
    )?;
    write(
        &vault,
        "rendering",
        "Validate render_draft destinations",
        "render_draft validates Wikilinks paths before publication.",
        MemoryScope::Global,
    )?;
    for (id, project) in [
        ("foreign", "beta"),
        ("pending", "alpha"),
        ("stale", "alpha"),
    ] {
        write(
            &vault,
            id,
            "Engine constructs safe Wikilinks",
            "Wikilinks resolve validated paths to stable IDs.",
            MemoryScope::Project {
                project_id: project.into(),
            },
        )?;
    }
    let search = Search::new(&root, &vault)?;
    search.rebuild()?;
    state.mark_note_pending("pending", "fixture")?;
    fs::write(vault.join("stale.md"), "changed")?;
    let c = candidate(
        "Engine constructs safe Wikilinks from known note identities",
        "render_draft resolves validated paths to stable IDs.",
    );
    assert!(
        search
            .search(&c.title, &Scope::new("alpha"), 3)?
            .results
            .is_empty(),
        "Legacy AND query must reproduce the miss"
    );
    let hits = neighbors(&search, "alpha", &[c])?;
    let ids: Vec<_> = hits.iter().map(|h| h.metadata.id.as_str()).collect();
    assert!(ids.contains(&"links"));
    assert!(ids.contains(&"rendering"));
    assert!(
        !ids.iter()
            .any(|id| ["foreign", "pending", "stale"].contains(id))
    );
    Ok(())
}

#[test]
fn identifier_claims_empty_queries_and_rank_fusion_are_deterministic() -> Result<()> {
    let d = tempdir()?;
    let vault = d.path().join("vault");
    fs::create_dir(&vault)?;
    State::open(&d.path().join("state.sqlite"))?;
    write(
        &vault,
        "identifier",
        "Capture implementation",
        "Call State::claim_store before sync_all.",
        MemoryScope::Project {
            project_id: "alpha".into(),
        },
    )?;
    write(
        &vault,
        "a-broad",
        "Allocator overview",
        "Allocator handles jobs.",
        MemoryScope::Global,
    )?;
    write(
        &vault,
        "z-specific",
        "Allocator tokens",
        "Allocator handles tokens with fencing.",
        MemoryScope::Global,
    )?;
    let search = Search::new(d.path(), &vault)?;
    search.rebuild()?;
    let mut c = candidate(
        "New behavior differs from previous wording",
        "State::claim_store uses sync_all.",
    );
    c.conditions.clear();
    assert_eq!(
        neighbors(&search, "alpha", &[c])?[0].metadata.id,
        "identifier"
    );
    let mut c = candidate("Allocator tokens fencing", "No conclusion.");
    c.conditions.clear();
    let hits = neighbors(&search, "alpha", std::slice::from_ref(&c))?;
    assert_eq!(hits[0].metadata.id, "z-specific");
    assert_eq!(
        hits.iter().map(|h| &h.metadata.id).collect::<Vec<_>>(),
        neighbors(&search, "alpha", &[c])?
            .iter()
            .map(|h| &h.metadata.id)
            .collect::<Vec<_>>()
    );
    let mut c = candidate("!!!", "the and or");
    c.conditions.clear();
    assert!(neighbors(&search, "alpha", &[c])?.is_empty());
    let mut c = candidate("Unfindablexyz", "unfindablexyz");
    c.conditions.clear();
    assert!(neighbors(&search, "alpha", &[c])?.is_empty());
    Ok(())
}
#[test]
fn neighbor_limits_and_round_robin_preserve_space_for_other_candidates() -> Result<()> {
    let d = tempdir()?;
    let vault = d.path().join("vault");
    fs::create_dir(&vault)?;
    State::open(&d.path().join("state.sqlite"))?;
    let mut candidates = Vec::new();
    for n in 0..20 {
        let term = format!("protocol{n:02}");
        for hit in 0..6 {
            write(
                &vault,
                &format!("note{n:02}-{hit}"),
                &term,
                &term,
                MemoryScope::Global,
            )?;
        }
        let mut c = candidate(&term, &term);
        c.conditions.clear();
        candidates.push(c);
    }
    let search = Search::new(d.path(), &vault)?;
    search.rebuild()?;
    assert_eq!(neighbors(&search, "alpha", &candidates[..1])?.len(), 3);
    let hits = neighbors(&search, "alpha", &candidates)?;
    assert_eq!(hits.len(), 10);
    for n in 0..10 {
        assert!(
            hits.iter()
                .any(|h| h.metadata.title == format!("protocol{n:02}"))
        );
    }
    candidates.push(candidates[0].clone());
    assert!(neighbors(&search, "alpha", &candidates).is_err());
    Ok(())
}
