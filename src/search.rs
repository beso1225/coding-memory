//! Rebuildable FTS index with authoritative-file and durable publication checks.
use crate::{
    domain::hash,
    evidence,
    notes::{self, MemoryScope, Metadata, Note, Status},
    state::State,
};
use anyhow::{Result, ensure};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params, params_from_iter};
use serde::Serialize;
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Debug, Clone)]
pub struct Scope {
    pub project_id: String,
    pub other_projects: Vec<String>,
    pub include_historical: bool,
}
impl Scope {
    pub fn new(project: &str) -> Self {
        Self {
            project_id: project.into(),
            other_projects: Vec::new(),
            include_historical: false,
        }
    }
    fn validate(&self) -> Result<()> {
        notes::identity(&self.project_id)?;
        ensure!(self.other_projects.len() <= 16, "Too many other projects");
        for p in &self.other_projects {
            notes::identity(p)?;
        }
        Ok(())
    }
    fn permits(&self, meta: &Metadata) -> bool {
        (self.include_historical || meta.status == Status::Active)
            && match &meta.scope {
                MemoryScope::Global => true,
                MemoryScope::Project { project_id } => {
                    project_id == &self.project_id || self.other_projects.contains(project_id)
                }
            }
    }
}
#[derive(Debug, Serialize)]
pub struct Hit {
    pub metadata: Metadata,
    pub score: f64,
    pub revision: String,
}
#[derive(Debug, Serialize)]
pub struct Results {
    pub results: Vec<Hit>,
    pub omitted_unavailable: usize,
    pub scan_limit_reached: bool,
}
#[derive(Debug, Serialize)]
pub struct Read {
    pub metadata: Metadata,
    pub markdown: String,
    pub revision: String,
    pub evidence_available: Vec<bool>,
}
#[derive(Debug, Serialize)]
pub struct InvalidNote {
    pub path: String,
    pub reason: String,
}
#[derive(Debug, Serialize)]
pub struct Rebuild {
    pub indexed: usize,
    pub invalid: Vec<InvalidNote>,
}
#[derive(Debug, Clone)]
pub struct Search {
    root: PathBuf,
    vault: PathBuf,
}
impl Search {
    pub fn new(root: &Path, vault: &Path) -> Result<Self> {
        let root = root.canonicalize()?;
        let vault = vault.canonicalize()?;
        ensure!(
            vault.is_dir() && !root.starts_with(&vault),
            "Engine state must be outside the vault"
        );
        State::open(&root.join("state.sqlite"))?;
        Ok(Self { root, vault })
    }
    fn index(&self) -> Result<Connection> {
        let db = Connection::open_with_flags(
            self.root.join("search.sqlite"),
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        db.busy_timeout(Duration::from_secs(5))?;
        let (version, vault): (i64, String) =
            db.query_row("SELECT version,vault FROM index_metadata", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })?;
        ensure!(
            version == 1 && Path::new(&vault) == self.vault,
            "Index schema/vault mismatch; rebuild explicitly"
        );
        Ok(db)
    }
    fn pending(&self, id: &str) -> Result<bool> {
        let db = Connection::open_with_flags(
            self.root.join("state.sqlite"),
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        db.busy_timeout(Duration::from_secs(5))?;
        Ok(db.query_row(
            "SELECT EXISTS(SELECT 1 FROM pending_notes WHERE note_id=?1)",
            [id],
            |r| r.get(0),
        )?)
    }
    fn file(&self, relative: &str) -> Result<String> {
        let path = self.vault.join(relative);
        ensure!(
            !fs::symlink_metadata(&path)?.file_type().is_symlink(),
            "Symlinked note is unavailable"
        );
        let canonical = path.canonicalize()?;
        ensure!(
            canonical.starts_with(&self.vault) && canonical.is_file(),
            "Note escapes vault"
        );
        Ok(String::from_utf8(evidence::read_bounded(
            &canonical,
            notes::MAX_NOTE_BYTES,
        )?)?)
    }
    pub fn rebuild(&self) -> Result<Rebuild> {
        let mut stack = vec![self.vault.clone()];
        let mut files = Vec::new();
        let mut visited = 0;
        while let Some(dir) = stack.pop() {
            for entry in fs::read_dir(dir)? {
                let entry = entry?;
                visited += 1;
                ensure!(visited <= 10000, "Vault exceeds entry limit");
                let kind = entry.file_type()?;
                let name = entry.file_name();
                if name.to_string_lossy().starts_with('.') || kind.is_symlink() {
                    continue;
                }
                if kind.is_dir() {
                    stack.push(entry.path());
                } else if kind.is_file() && entry.path().extension().is_some_and(|s| s == "md") {
                    files.push(entry.path());
                }
            }
        }
        files.sort();
        let mut report = Rebuild {
            indexed: 0,
            invalid: Vec::new(),
        };
        let mut parsed = Vec::new();
        let mut count = HashMap::new();
        let mut total = 0;
        for path in files {
            let relative = path
                .strip_prefix(&self.vault)?
                .to_str()
                .ok_or_else(|| anyhow::anyhow!("Non-UTF8 note path"))?
                .to_owned();
            let decoded = (|| -> Result<(Note, String)> {
                let markdown = self.file(&relative)?;
                total += markdown.len();
                ensure!(total <= 64 * 1024 * 1024, "Vault exceeds byte limit");
                let note = notes::parse(&markdown)?;
                Ok((note, hash(markdown.as_bytes())))
            })();
            match decoded {
                Ok((note, revision)) => {
                    *count.entry(note.metadata.id.clone()).or_insert(0) += 1;
                    parsed.push((relative, note, revision));
                }
                Err(error) => report.invalid.push(InvalidNote {
                    path: relative,
                    reason: error.to_string(),
                }),
            }
        }
        // A resource failure aborts the rebuild rather than replacing the index with a prefix.
        ensure!(total <= 64 * 1024 * 1024, "Vault exceeds byte limit");
        let mut db = Connection::open(self.root.join("search.sqlite"))?;
        db.busy_timeout(Duration::from_secs(5))?;
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; CREATE TABLE IF NOT EXISTS index_metadata(version INTEGER NOT NULL,vault TEXT NOT NULL);")?;
        let version: Option<i64> = db
            .query_row("SELECT version FROM index_metadata", [], |r| r.get(0))
            .optional()?;
        ensure!(version.is_none_or(|v| v == 1), "Unsupported search schema");
        let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        tx.execute_batch("CREATE TABLE IF NOT EXISTS indexed_notes(id TEXT PRIMARY KEY,path TEXT NOT NULL,revision TEXT NOT NULL,metadata TEXT NOT NULL,project TEXT,status TEXT NOT NULL);
            CREATE VIRTUAL TABLE IF NOT EXISTS memory_fts USING fts5(id UNINDEXED,title,summary,body,tokenize='unicode61 remove_diacritics 2');
            DELETE FROM indexed_notes; DELETE FROM memory_fts; DELETE FROM index_metadata;")?;
        tx.execute(
            "INSERT INTO index_metadata VALUES(1,?1)",
            [self
                .vault
                .to_str()
                .ok_or_else(|| anyhow::anyhow!("Non-UTF8 vault"))?],
        )?;
        for (path, note, revision) in parsed {
            if count[&note.metadata.id] != 1 {
                report.invalid.push(InvalidNote {
                    path,
                    reason: "Duplicate memory identity".into(),
                });
                continue;
            }
            let project = match &note.metadata.scope {
                MemoryScope::Project { project_id } => Some(project_id.as_str()),
                MemoryScope::Global => None,
            };
            let status = if note.metadata.status == Status::Active {
                "active"
            } else {
                "superseded"
            };
            tx.execute(
                "INSERT INTO indexed_notes VALUES(?1,?2,?3,?4,?5,?6)",
                params![
                    note.metadata.id,
                    path,
                    revision,
                    serde_json::to_string(&note.metadata)?,
                    project,
                    status
                ],
            )?;
            tx.execute(
                "INSERT INTO memory_fts(id,title,summary,body) VALUES(?1,?2,?3,?4)",
                params![
                    note.metadata.id,
                    note.metadata.title,
                    note.metadata.summary,
                    note.body
                ],
            )?;
            report.indexed += 1;
        }
        tx.commit()?;
        Ok(report)
    }
    pub fn search(&self, query: &str, scope: &Scope, limit: usize) -> Result<Results> {
        scope.validate()?;
        ensure!((1..=10).contains(&limit), "Result limit must be 1..10");
        let query = literal_query(query)?;
        let db = self.index()?;
        let mut projects = vec![scope.project_id.clone()];
        projects.extend(scope.other_projects.iter().cloned());
        let placeholders = (0..projects.len())
            .map(|_| "?")
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT n.path,n.revision,n.metadata,bm25(memory_fts,0.0,4.0,2.0,1.0) FROM memory_fts JOIN indexed_notes n ON n.id=memory_fts.id WHERE memory_fts MATCH ? AND (n.project IS NULL OR n.project IN ({placeholders})) AND (? OR n.status='active') ORDER BY bm25(memory_fts,0.0,4.0,2.0,1.0),n.id LIMIT 1001"
        );
        let mut params: Vec<rusqlite::types::Value> = vec![query.into()];
        params.extend(projects.into_iter().map(Into::into));
        params.push((scope.include_historical as i64).into());
        let mut statement = db.prepare(&sql)?;
        let mut rows = statement.query(params_from_iter(params))?;
        let mut response = Results {
            results: Vec::new(),
            omitted_unavailable: 0,
            scan_limit_reached: false,
        };
        let mut scanned = 0;
        while let Some(row) = rows.next()? {
            scanned += 1;
            if scanned > 1000 {
                response.scan_limit_reached = true;
                break;
            }
            let path: String = row.get(0)?;
            let revision: String = row.get(1)?;
            let meta: String = row.get(2)?;
            let metadata: Metadata = serde_json::from_str(&meta)?;
            if self.pending(&metadata.id)?
                || self
                    .file(&path)
                    .map_or(true, |s| hash(s.as_bytes()) != revision)
            {
                response.omitted_unavailable += 1;
                continue;
            }
            if !scope.permits(&metadata) {
                continue;
            }
            response.results.push(Hit {
                metadata,
                score: row.get(3)?,
                revision,
            });
            if serde_json::to_vec(&response)?.len() > 64 * 1024 {
                response.results.pop();
                response.scan_limit_reached = true;
                break;
            }
            if response.results.len() == limit {
                break;
            }
        }
        Ok(response)
    }
    pub fn read(&self, id: &str, scope: &Scope) -> Result<Read> {
        notes::identity(id)?;
        scope.validate()?;
        ensure!(!self.pending(id)?, "Note pending publication/recovery");
        let db = self.index()?;
        let (path, revision): (String, String) = db.query_row(
            "SELECT path,revision FROM indexed_notes WHERE id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let markdown = self.file(&path)?;
        ensure!(
            hash(markdown.as_bytes()) == revision,
            "Note requires index refresh"
        );
        let note = notes::parse(&markdown)?;
        ensure!(
            note.metadata.id == id && scope.permits(&note.metadata),
            "Note outside requested scope"
        );
        ensure!(!self.pending(id)?, "Note pending publication/recovery");
        let evidence_available = note
            .metadata
            .evidence
            .iter()
            .map(|r| {
                evidence::load(&self.root, &r.bundle)
                    .is_ok_and(|e| e.items.iter().any(|i| i["id"] == r.item))
            })
            .collect();
        Ok(Read {
            metadata: note.metadata,
            markdown,
            revision,
            evidence_available,
        })
    }
}
/// Human input is literal text, never FTS expressions or interpolated SQL.
pub fn literal_query(query: &str) -> Result<String> {
    ensure!(
        query.len() <= 1024 && !query.contains('\0'),
        "Invalid or oversized search query"
    );
    let terms = query
        .split_whitespace()
        .filter(|t| t.chars().any(char::is_alphanumeric))
        .collect::<Vec<_>>();
    ensure!(
        !terms.is_empty() && terms.len() <= 16,
        "Query needs 1..16 searchable terms"
    );
    let mut seen = HashSet::new();
    Ok(terms
        .into_iter()
        .filter(|t| seen.insert(t.to_lowercase()))
        .map(|t| format!("\"{}\"", t.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" AND "))
}
