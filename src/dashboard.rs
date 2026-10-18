//! Metadata-only, read-only HTML snapshots. No models or Vault access.
use crate::{ingestion, state::State};
use anyhow::Result;
use std::{fmt::Write, path::Path};

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

// UTC only; keep snapshots usable without JavaScript or a timezone dependency.
fn timestamp(seconds: i64) -> String {
    if !(0..=253_402_300_799).contains(&seconds) {
        return format!("Unavailable (Unix seconds: {seconds})");
    }
    let mut days = seconds / 86_400;
    let mut year = 1970;
    let leap = |y| y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    loop {
        let length = if leap(year) { 366 } else { 365 };
        if days < length {
            break;
        }
        days -= length;
        year += 1;
    }
    let lengths = [
        31,
        if leap(year) { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut month = 1;
    for length in lengths {
        if days < length {
            break;
        }
        days -= length;
        month += 1;
    }
    format!(
        "{year:04}-{month:02}-{:02} {:02}:{:02}:{:02} UTC",
        days + 1,
        seconds / 3600 % 24,
        seconds / 60 % 60,
        seconds % 60
    )
}

/// Only render explicitly selected metadata. Database errors and arbitrary
/// reason/provenance strings may contain private data and are never printed.
pub fn render(root: &Path) -> String {
    render_mode(root, false)
}

pub(crate) fn render_live(root: &Path) -> String {
    render_mode(root, true)
}

fn render_mode(root: &Path, live: bool) -> String {
    let mut html = include_str!("dashboard.html").to_owned();
    let _ = write!(
        html,
        "<p class=timestamp>Snapshot obtained: {}. Worker activity can change immediately.</p>",
        timestamp(ingestion::now())
    );
    if live {
        html.push_str("<p>Updates automatically every 5 seconds while this local viewer is running. Background tabs may update less frequently.</p>");
    } else {
        html.push_str("<p>Static snapshot. Regenerate with <code>coding-memory --state-dir STATE dashboard &gt; status.html</code>, then reload this file. Browser reload alone does not obtain new data.</p>");
    }
    let _ = write!(
        html,
        "<p>State directory: <code>{}</code></p>",
        escape(&root.display().to_string())
    );
    match State::open_read_only(&root.join("state.sqlite")) {
        Ok(state) => match state.db.unchecked_transaction() {
            Ok(_snapshot) => {
                worker(&mut html, &state);
                for (table, title) in [
                    ("runs", "Capture"),
                    ("extraction_jobs", "Extraction"),
                    ("candidates", "Candidates"),
                    (
                        "consolidation_jobs",
                        "Consolidation / review / Vault application",
                    ),
                ] {
                    section(&mut html, title, grouped(&state, table));
                }
                section(&mut html, "Needs attention", problems(&state));
                section(&mut html, "Recent extraction outcomes", extractions(&state));
                section(&mut html, "Published memory records", published(&state));
            }
            Err(_) => unavailable(&mut html),
        },
        Err(_) => unavailable(&mut html),
    }
    html.push_str("<section><h2>Recovery reference</h2><p>Inspect status and fix the cause before deliberately retrying. These commands are reference text; this page has no controls.</p><ul><li>Capture: <code>status</code>, <code>capture-diagnose</code>, <code>retry STORE THREAD TURN</code>.</li><li>Extraction: <code>extraction-status --project-id PROJECT</code>, <code>extraction-retry STORE THREAD TURN</code>.</li><li>Consolidation: <code>consolidation-status</code>, <code>consolidation-preflight --vault VAULT --project-id PROJECT --batch BATCH</code>, <code>consolidation-retry BATCH</code>.</li><li>Vault conflicts: inspect file revisions, then <code>vault-recover --vault VAULT BATCH</code>.</li><li>Reader verification: <code>runtime-check-status</code>, <code>runtime-verify --binary BINARY</code>.</li></ul><p>Prefix commands with <code>coding-memory --state-dir STATE</code>. See <a href=\"https://github.com/beso1225/coding-memory/blob/main/docs/worker.md#inspection-and-recovery\">worker recovery documentation</a>.</p></section></main></body></html>");
    html
}

fn unavailable(html: &mut String) {
    html.push_str("<p class=attention data-unavailable>Unavailable — status could not be read. This does not mean zero work. Check the state location, schema, permissions and CLI status.</p>");
}

fn section(html: &mut String, title: &str, content: Result<String>) {
    let _ = write!(html, "<section><h2>{}</h2>", escape(title));
    match content {
        Ok(value) => html.push_str(&value),
        Err(_) => unavailable(html),
    }
    html.push_str("</section>");
}

fn worker(html: &mut String, state: &State) {
    let content = (|| -> Result<String> {
        // Reuse the CLI's actual flock observation and typed cycle report.
        let status = state.worker_status()?;
        let mut out = format!(
            "<p class=timestamp>Worker lock: {}</p><p>Lock ownership indicates a process owns the worker, not that generation is currently executing.</p>",
            if status["active"] == true {
                "active"
            } else {
                "inactive"
            }
        );
        if status["last_cycle"].is_null() {
            out.push_str("<p>No recorded cycle.</p>");
        } else {
            let r: crate::worker::Report = serde_json::from_value(status["last_cycle"].clone())?;
            let _ = write!(
                out,
                "<p>Last cycle started: {}. Completed: {}. Interrupted: {}.</p><p>Attempts: capture {}; extraction {}; consolidation batches {}. Recovered claims: {}. Reported failures: {}.</p>",
                timestamp(r.started_at),
                r.completed_at
                    .map(timestamp)
                    .unwrap_or_else(|| "not recorded".into()),
                r.interrupted,
                r.capture_attempts,
                r.extraction_attempts,
                r.consolidation_batches,
                r.recovered,
                r.failures.len()
            );
            for failure in r.failures {
                let stage = match failure.stage.as_str() {
                    "reader_start_failed" | "reader_close_failed" => {
                        "Reader process failed; inspect runtime verification and worker configuration"
                    }
                    "runtime_validation_failed" => {
                        "Reader verification failed; inspect runtime-check-status"
                    }
                    "capture_failed" | "reconciliation_failed" => {
                        "Capture failed; inspect status and capture-diagnose"
                    }
                    "extraction_failed" | "extraction_enqueue_failed" => {
                        "Extraction failed; inspect extraction-status"
                    }
                    "consolidation_failed" => "Consolidation failed; inspect consolidation-status",
                    "application_conflict"
                    | "application_recovery_failed"
                    | "recovery_vault_mismatch" => {
                        "Vault recovery requires inspection of consolidation-status and configured Vault"
                    }
                    "project_unavailable" | "project_unregistered" => {
                        "Project unavailable; inspect project registration and worker sources"
                    }
                    _ => "Unrecognized stage failure; inspect CLI status locally",
                };
                let _ = write!(
                    out,
                    "<p class=attention>{}: {stage}.</p>",
                    escape(failure.project.as_deref().unwrap_or("All projects"))
                );
            }
        }
        out.push_str("<p>Times are UTC. A completed cycle can include failures or held work. Attempts are not published-memory counts.</p>");
        Ok(out)
    })();
    section(html, "Worker", content);
}

fn table(headers: &[&str], rows: Vec<Vec<String>>, empty: &str) -> String {
    if rows.is_empty() {
        return format!("<p>{}</p>", escape(empty));
    }
    let mut out = "<div class=scroll><table><thead><tr>".to_owned();
    for header in headers {
        let _ = write!(out, "<th scope=col>{}</th>", escape(header));
    }
    out.push_str("</tr></thead><tbody>");
    for row in rows {
        out.push_str("<tr>");
        for (header, cell) in headers.iter().zip(row) {
            let class = if matches!(*header, "State" | "Count" | "Jobs" | "Batch") {
                " class=nowrap"
            } else {
                ""
            };
            let _ = write!(out, "<td{class}>{}</td>", escape(&cell));
        }
        out.push_str("</tr>");
    }
    out.push_str("</tbody></table></div>");
    out
}

fn meaning(table: &str, state: &str) -> &'static str {
    match (table, state) {
        ("runs", "ready") => return "Capture complete; available for extraction",
        ("runs", "recorded") => return "Terminal run recorded; inspect outcome",
        _ => {}
    }
    match state {
        "succeeded_empty" => "No candidates — extraction succeeded",
        "succeeded" => "Extraction succeeded; candidates staged",
        "queued" => "Waiting (may be delayed for retry)",
        "held" => "Held; inspect reason before deliberate retry",
        "exhausted" => "Automatic retry limit reached",
        "failed" => "Failed; inspect status",
        "conflict" => "Vault revisions conflict; inspect before recovery",
        "applying" => "Vault application incomplete; recovery pending",
        "applied" => "Batch application complete",
        "running" | "capturing" | "generating" => {
            "Claimed; may need lease recovery after interruption"
        }
        "ready" => "Waiting for consolidation trigger",
        "reserved" => "Reserved by consolidation",
        "consumed" => "Candidate consumed; not a published-memory count",
        "captured" => "Capture complete",
        _ => "See CLI status semantics",
    }
}

fn grouped(state: &State, name: &str) -> Result<String> {
    // Table names come only from the fixed list in render().
    let outcome = if name == "runs" { "outcome" } else { "NULL" };
    let mut q = state.db.prepare(&format!(
        "SELECT project,state,count(*),{outcome} FROM {name} GROUP BY project,state,{outcome} ORDER BY project,state,{outcome}"
    ))?;
    let rows = q
        .query_map([], |r| {
            let status: String = r.get(1)?;
            Ok(vec![
                r.get(0)?,
                status.clone(),
                r.get::<_, i64>(2)?.to_string(),
                meaning(name, &status).into(),
                r.get::<_, Option<String>>(3)?.unwrap_or_else(|| "—".into()),
            ])
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(table(
        &["Project", "State", "Count", "Meaning", "Run outcome"],
        rows,
        "0 records (data available).",
    ))
}

fn reason(code: Option<&str>) -> &'static str {
    match code {
        Some("held_external_policy") => "External transmission not allowed; inspect project policy",
        Some("generation_auth_unavailable") => {
            "Authentication unavailable; inspect the Engine generation login"
        }
        Some(
            "generation_timeout"
            | "generation_process_failure"
            | "generation_lease_expired"
            | "worker_interrupted",
        ) => "Generation interrupted or failed; check runtime and retry limits",
        Some(
            "held_context_too_large"
            | "held_context_unavailable"
            | "held_invalid_or_oversized_evidence",
        ) => {
            "Evidence/context unavailable or too large; inspect consolidation preflight or extraction status"
        }
        Some("revision_conflict") => {
            "Revision conflict; inspect Vault revisions before vault-recover"
        }
        Some("held_invalid_proposal" | "held_invalid_review" | "held_plan_or_policy_conflict") => {
            "Consolidation proposal/review rejected or policy conflict; inspect consolidation-status before retry"
        }
        Some(
            "generation_invalid_output"
            | "generation_output_limit"
            | "generation_tool_attempt"
            | "generation_unsupported_runtime"
            | "generation_isolation_unavailable",
        ) => "Generation output/runtime rejected; inspect generation configuration",
        Some("source_excluded") => "Source excluded by collection policy",
        Some(
            "missing_history_version"
            | "unsupported_history_version"
            | "unsupported_history_method"
            | "invalid_history_page_data"
            | "invalid_history_page_cursor"
            | "invalid_turn_identity"
            | "unsupported_turn_status"
            | "missing_turn_start"
            | "invalid_turn_timestamps"
            | "invalid_item_identity"
            | "missing_item_projection"
            | "item_turn_mismatch"
            | "missing_source_path"
            | "missing_turn_context"
            | "missing_turn_cwd"
            | "conflicting_turn_context",
        ) => {
            "Capture history unsupported or incomplete; inspect capture-diagnose and App capture documentation"
        }
        None => "No recorded reason; inspect stage status",
        _ => "Other recorded reason (details withheld); inspect stage status locally",
    }
}

fn problems(state: &State) -> Result<String> {
    let mut rows = Vec::new();
    for (name, title) in [
        ("runs", "Capture"),
        ("extraction_jobs", "Extraction"),
        ("candidates", "Candidates"),
        ("consolidation_jobs", "Consolidation / Vault"),
    ] {
        let reason_column = if name == "candidates" {
            "NULL"
        } else {
            "reason"
        };
        let outcome = if name == "runs" { "outcome" } else { "NULL" };
        let failed_runs = if name == "runs" {
            "OR outcome IN ('failed','interrupted')"
        } else {
            ""
        };
        let mut q = state.db.prepare(&format!("SELECT project,state,{reason_column},count(*),{outcome} FROM {name} WHERE state IN ('held','exhausted','failed','conflict','applying') {} {failed_runs} GROUP BY project,state,{reason_column},{outcome} ORDER BY count(*) DESC,project,state LIMIT 100", if name == "candidates" { "" } else { "OR reason IS NOT NULL" }))?;
        for row in q.query_map([], |r| {
            Ok(vec![
                title.into(),
                r.get(0)?,
                match r.get::<_, Option<String>>(4)?.as_deref() {
                    Some("failed") => "recorded (failed)".into(),
                    Some("interrupted") => "recorded (interrupted)".into(),
                    _ => r.get(1)?,
                },
                r.get::<_, i64>(3)?.to_string(),
                reason(r.get::<_, Option<String>>(2)?.as_deref()).into(),
            ])
        })? {
            rows.push(row?);
        }
    }
    Ok(format!(
        "<p>Up to 100 grouped entries per stage, including delayed jobs with recorded reasons. Use stage status commands for individual job identifiers and full details.</p>{}",
        table(
            &["Stage", "Project", "State", "Count", "Reason / guidance"],
            rows,
            "No recorded items requiring attention."
        )
    ))
}

fn extractions(state: &State) -> Result<String> {
    let mut q = state.db.prepare("SELECT project,state,finished_at,count(*) FROM extraction_jobs WHERE state IN ('succeeded','succeeded_empty') GROUP BY project,state,finished_at ORDER BY finished_at DESC,project,state LIMIT 30")?;
    let rows = q
        .query_map([], |r| {
            let status: String = r.get(1)?;
            Ok(vec![
                r.get(0)?,
                status.clone(),
                meaning("extraction_jobs", &status).into(),
                r.get::<_, Option<i64>>(2)?
                    .map(timestamp)
                    .unwrap_or_else(|| "Unavailable".into()),
                r.get::<_, i64>(3)?.to_string(),
            ])
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(format!(
        "<p>Latest 30 groups by extraction finish time (UTC).</p>{}",
        table(
            &["Project", "State", "Outcome", "Finished", "Jobs"],
            rows,
            "0 successful extraction records (data available)."
        )
    ))
}

fn published(state: &State) -> Result<String> {
    let mut q = state.db.prepare("SELECT j.project,j.id,v.note_id FROM consolidation_jobs j JOIN vault_changes v ON v.batch=j.id WHERE j.state='applied' AND v.applied=1 ORDER BY j.id DESC,v.note_id LIMIT 30")?;
    let rows = q
        .query_map([], |r| {
            Ok(vec![r.get(0)?, r.get::<_, i64>(1)?.to_string(), r.get(2)?])
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(format!(
        "<p>Latest 30 applied note records by batch ID, including updates to existing memories. Publication time is not recorded; these are not unique new-memory counts. Current Vault content is not inspected.</p>{}",
        table(
            &["Project", "Batch", "Memory ID"],
            rows,
            "0 applied note records (data available)."
        )
    ))
}

#[cfg(test)]
mod tests {
    use super::timestamp;

    #[test]
    fn utc_timestamps_handle_leap_years_and_invalid_values() {
        assert_eq!(timestamp(0), "1970-01-01 00:00:00 UTC");
        assert_eq!(timestamp(951_782_400), "2000-02-29 00:00:00 UTC");
        assert_eq!(timestamp(4_107_542_400), "2100-03-01 00:00:00 UTC");
        assert_eq!(timestamp(253_402_300_799), "9999-12-31 23:59:59 UTC");
        assert!(timestamp(-1).starts_with("Unavailable"));
    }
}
