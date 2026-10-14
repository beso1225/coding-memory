//! Bounded lexical neighbors for consolidation; public MCP queries remain literal AND.
use crate::{
    extraction::Candidate,
    search::{Hit, MAX_QUERY_BYTES, Scope, Search},
};
use anyhow::{Result, ensure};
use std::collections::{HashMap, HashSet};

const PER_CANDIDATE: usize = 3;
const TOTAL: usize = 10;
const QUERY_HITS: usize = 5;
const STOPWORDS: &str = "a an the and or of to in on for from with by as at is are be been being this that these those it its when where how which who while before after only must should can could may might will would use uses using each all any into not no do does done than then also have has had their our your we they through";

fn terms(text: &str) -> impl Iterator<Item = String> + '_ {
    text.split(|c: char| !c.is_alphanumeric() && !matches!(c, '_' | ':' | '/' | '.' | '-'))
        .map(|s| {
            s.trim_matches(|c: char| !c.is_alphanumeric())
                .to_lowercase()
        })
        .filter(|s| {
            (3..=MAX_QUERY_BYTES).contains(&s.len())
                && !STOPWORDS.split_whitespace().any(|w| w == s)
        })
}
fn queries(candidate: &Candidate) -> Vec<String> {
    let mut queries = Vec::new();
    let title = candidate
        .title
        .split_whitespace()
        .filter(|chunk| chunk.chars().any(char::is_alphanumeric))
        .take(8)
        .map(str::to_lowercase)
        .collect::<Vec<_>>()
        .join(" ");
    if !title.is_empty() && title.len() <= MAX_QUERY_BYTES {
        queries.push(title);
    }
    let mut seen = HashSet::new();
    // Field quotas keep a verbose title from hiding a technical term in the claim.
    let conditions = candidate.conditions.join(" ");
    for (text, quota) in [
        (&candidate.title, 4),
        (&candidate.claim, 3),
        (&conditions, 1),
    ] {
        let selected: Vec<_> = terms(text)
            .filter(|term| seen.insert(term.clone()) && !queries.contains(term))
            .take(quota)
            .collect();
        queries.extend(selected);
    }
    queries
}

/// Rank short-query results per candidate, then round-robin the top three lists.
/// At most nine searches/candidate (180/batch), five hits/query, ten unique neighbors.
/// Reciprocal ranks are fused; BM25 scores from different queries are not comparable.
pub fn neighbors(search: &Search, project: &str, candidates: &[Candidate]) -> Result<Vec<Hit>> {
    ensure!(candidates.len() <= 20, "Too many retrieval candidates");
    let scope = Scope::new(project);
    let mut cache: HashMap<String, Vec<(String, u64)>> = HashMap::new();
    let mut pool = HashMap::new();
    let mut ranked = Vec::new();
    for candidate in candidates {
        let mut scores: HashMap<String, u64> = HashMap::new();
        for query in queries(candidate) {
            if !cache.contains_key(&query) {
                let mut ranks = Vec::new();
                for (rank, hit) in search
                    .search(&query, &scope, QUERY_HITS)?
                    .results
                    .into_iter()
                    .enumerate()
                {
                    let id = hit.metadata.id.clone();
                    ranks.push((id.clone(), 1_000_000 / (60 + rank as u64)));
                    pool.entry(id).or_insert(hit);
                }
                cache.insert(query.clone(), ranks);
            }
            for (id, score) in &cache[&query] {
                *scores.entry(id.clone()).or_default() += score;
            }
        }
        let mut scores: Vec<_> = scores.into_iter().collect();
        scores.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        ranked.push(
            scores
                .into_iter()
                .take(PER_CANDIDATE)
                .map(|(id, _)| id)
                .collect::<Vec<_>>(),
        );
    }
    let mut neighbors = Vec::new();
    for rank in 0..PER_CANDIDATE {
        for list in &ranked {
            if let Some(id) = list.get(rank)
                && let Some(hit) = pool.remove(id)
            {
                neighbors.push(hit);
                if neighbors.len() == TOTAL {
                    return Ok(neighbors);
                }
            }
        }
    }
    Ok(neighbors)
}
