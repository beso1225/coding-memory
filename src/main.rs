use anyhow::{Result, ensure};
use clap::{Parser, Subcommand};
use coding_memory::{
    codex::Client,
    domain::RunKey,
    evidence, extraction_worker,
    generation::{self, CodexExec},
    ingestion,
    state::State,
};
use coding_memory::{
    harness::Harness,
    mcp,
    search::{Scope, Search},
};
use std::{
    io::{self, Read},
    path::PathBuf,
};

#[derive(Parser)]
#[command(version, about = "Durable coding-agent evidence capture")]
struct Cli {
    /// Explicit local state location, outside software and vault repositories.
    #[arg(long)]
    state_dir: PathBuf,
    #[command(subcommand)]
    command: Commands,
}

#[cfg(test)]
mod tests {
    use super::Cli;
    use clap::CommandFactory;
    #[test]
    fn command_definition_is_valid() {
        Cli::command().debug_assert();
    }
    #[test]
    fn invalid_timeouts_are_rejected_during_parsing() {
        use clap::Parser;
        for value in ["0", "601", "18446744073709551615"] {
            assert!(
                Cli::try_parse_from([
                    "coding-memory",
                    "--state-dir",
                    "/unused",
                    "extract",
                    "--project-id",
                    "alpha",
                    "--timeout-seconds",
                    value
                ])
                .is_err()
            );
        }
        for value in ["1", "600"] {
            assert!(
                Cli::try_parse_from([
                    "coding-memory",
                    "--state-dir",
                    "/unused",
                    "extract",
                    "--project-id",
                    "alpha",
                    "--timeout-seconds",
                    value
                ])
                .is_ok()
            );
        }
    }
}
#[derive(Subcommand)]
enum Commands {
    Worker {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        once: bool,
    },
    WorkerStatus,
    /// Print HTML, or serve a read-only dashboard on 127.0.0.1.
    Dashboard {
        #[arg(long)]
        serve: bool,
        /// Local viewer port; 0 selects an available port.
        #[arg(long, default_value_t = 8765, requires = "serve")]
        port: u16,
    },
    RuntimeCheckStatus,
    RuntimeVerify {
        #[arg(long)]
        binary: PathBuf,
        #[arg(long)]
        retry: bool,
    },
    CaptureDiagnose {
        #[command(flatten)]
        runtime: Runtime,
        #[arg(long)]
        thread: String,
    },
    ServicePlist {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        output: PathBuf,
        #[arg(long)]
        executable: Option<PathBuf>,
        #[arg(long, default_value = "com.beso1225.coding-memory")]
        label: String,
    },
    Consolidate {
        #[arg(long)]
        vault: PathBuf,
        #[arg(long)]
        project_id: String,
        #[arg(long)]
        force: bool,
        #[arg(long,default_value_t=5,value_parser=clap::value_parser!(i64).range(1..=20))]
        min_ready: i64,
        #[arg(long,default_value_t=600,value_parser=clap::value_parser!(i64).range(1..=86400))]
        max_age_seconds: i64,
        #[arg(long, default_value = "codex")]
        codex_binary: PathBuf,
        #[arg(long, default_value = "gpt-6.1-sol")]
        model: String,
        #[arg(long, default_value = "gpt-6.1-sol")]
        review_model: String,
        #[arg(long,default_value_t=300,value_parser=clap::value_parser!(u64).range(1..=600))]
        timeout_seconds: u64,
        #[arg(long, hide = true)]
        loopback: Option<std::net::SocketAddr>,
    },
    /// Inspect explicitly selected held batches without models or queue changes.
    ConsolidationPreflight {
        #[arg(long)]
        vault: PathBuf,
        #[arg(long)]
        project_id: String,
        #[arg(long, required = true)]
        batch: Vec<i64>,
        /// Check sizing for local-only generation; never makes any model call.
        #[arg(long)]
        local_only: bool,
    },
    ConsolidationStatus,
    ConsolidationRetry {
        batch: i64,
    },
    VaultRecover {
        #[arg(long)]
        vault: PathBuf,
        batch: i64,
    },
    EvidenceCleanup {
        #[arg(long)]
        vault: PathBuf,
        #[arg(long,default_value_t=30,value_parser=clap::value_parser!(u64).range(30..=36500))]
        retention_days: u64,
        #[arg(long)]
        apply: bool,
    },
    /// Prepare an engine-owned Codex home; login remains an explicit human operation.
    GenerationSetup,
    /// Internal PreToolUse hook. Always exits with a deny status.
    #[command(hide = true)]
    GenerationDenyTool {
        #[arg(long)]
        marker_dir: PathBuf,
    },
    ExternalPolicy {
        #[arg(long)]
        project_id: String,
        #[arg(value_enum)]
        decision: ExternalDecision,
    },
    Extract {
        #[arg(long)]
        project_id: String,
        #[arg(long, default_value = "codex")]
        codex_binary: PathBuf,
        #[arg(long, default_value = "gpt-6.1-sol")]
        model: String,
        #[arg(long, default_value_t = 300, value_parser=clap::value_parser!(u64).range(1..=600))]
        timeout_seconds: u64,
        /// Synthetic validation only; must be a loopback address.
        #[arg(long, hide = true)]
        loopback: Option<std::net::SocketAddr>,
    },
    ExtractionStatus {
        #[arg(long)]
        project_id: String,
    },
    Candidates {
        #[arg(long)]
        project_id: String,
    },
    ExtractionRetry {
        store: String,
        thread: String,
        turn: String,
    },
    IndexRebuild {
        #[arg(long)]
        vault: PathBuf,
    },
    Search {
        #[arg(long)]
        vault: PathBuf,
        #[arg(long)]
        project_id: String,
        query: String,
        #[arg(long, default_value_t = 5)]
        limit: usize,
        #[arg(long)]
        other_project: Vec<String>,
        #[arg(long)]
        include_historical: bool,
    },
    Read {
        #[arg(long)]
        vault: PathBuf,
        #[arg(long)]
        project_id: String,
        id: String,
        #[arg(long)]
        other_project: Vec<String>,
        #[arg(long)]
        include_historical: bool,
    },
    Mcp {
        #[arg(long)]
        vault: PathBuf,
        #[arg(long)]
        project_id: String,
    },
    /// Print a harness MCP server entry for this project's state and Vault.
    McpConfig {
        #[arg(long)]
        vault: PathBuf,
        #[arg(long)]
        project_id: String,
        #[arg(long, value_enum)]
        harness: Harness,
        /// Executable recorded in the entry; defaults to this running binary.
        #[arg(long)]
        executable: Option<PathBuf>,
        /// Server name used by the harness; must be a bare identifier.
        #[arg(long, default_value = "coding_memory")]
        server_name: String,
    },
    Register(Runtime),
    Reconcile(Runtime),
    Capture(Runtime),
    /// Consume a bounded legacy notification argument or Stop/Interrupt JSON stdin.
    Enqueue {
        #[arg(long)]
        codex_home: PathBuf,
        event: Option<String>,
    },
    Status,
    /// Mark a dedicated generation store as engine-owned before generation starts.
    OwnStore {
        #[arg(long)]
        codex_home: PathBuf,
    },
    Retry {
        store: String,
        thread: String,
        turn: String,
    },
    Evidence {
        digest: String,
    },
}
#[derive(Clone, clap::ValueEnum)]
enum ExternalDecision {
    Allow,
    Deny,
}
#[derive(clap::Args)]
struct Runtime {
    #[arg(long)]
    codex_home: PathBuf,
    #[arg(long, default_value = "codex")]
    codex_binary: PathBuf,
    #[arg(long)]
    project: PathBuf,
}
#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    if let Commands::GenerationDenyTool { marker_dir } = &cli.command {
        let mut bytes = Vec::new();
        // Deny even when decoding or durable marker creation fails. No database opening.
        let _ = io::stdin()
            .take(65537)
            .read_to_end(&mut bytes)
            .and_then(|_| generation::deny_tool(marker_dir, &bytes).map_err(io::Error::other));
        eprintln!("Tool use is disabled for memory generation.");
        std::process::exit(2);
    }
    if let Commands::ConsolidationPreflight {
        vault,
        project_id,
        batch,
        local_only,
    } = &cli.command
    {
        let root = cli.state_dir.canonicalize()?;
        let state = State::open_read_only(&root.join("state.sqlite"))?;
        let report = coding_memory::consolidation_preflight::preflight(
            &state,
            &root,
            vault,
            project_id,
            batch,
            !local_only,
        )?;
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    if let Commands::Dashboard { serve, port } = &cli.command {
        if *serve {
            coding_memory::dashboard_server::run(cli.state_dir.clone(), *port).await?;
        } else {
            println!("{}", coding_memory::dashboard::render(&cli.state_dir));
        }
        return Ok(());
    }
    if let Commands::McpConfig {
        vault,
        project_id,
        harness,
        executable,
        server_name,
    } = &cli.command
    {
        let executable = match executable {
            Some(path) => path.clone(),
            None => std::env::current_exe()?,
        };
        println!(
            "{}",
            coding_memory::harness::mcp_config(
                *harness,
                server_name,
                &executable,
                &cli.state_dir,
                vault,
                project_id
            )?
        );
        return Ok(());
    }
    let root = evidence::private_directory(&cli.state_dir)?;
    let mut state = State::open(&root.join("state.sqlite"))?;
    match cli.command {
        Commands::Dashboard { .. } => unreachable!("Handled before state setup"),
        Commands::McpConfig { .. } => unreachable!("Handled before state setup"),
        Commands::ConsolidationPreflight { .. } => {
            unreachable!("Handled read-only before state setup")
        }
        Commands::RuntimeCheckStatus => println!(
            "{}",
            serde_json::to_string_pretty(&state.runtime_check_status()?)?
        ),
        Commands::RuntimeVerify { binary, retry } => {
            let identity = coding_memory::compatibility::identify(&binary, &root).await?;
            state.observe_runtime(&identity, ingestion::now())?;
            if retry {
                state.retry_runtime_check(&identity.digest, ingestion::now())?;
            }
            while coding_memory::compatibility::process_one(&root).await? {}
            let audit = state.observe_runtime(&identity, ingestion::now())?;
            ensure!(
                audit.validation == "verified",
                "Runtime verification did not pass"
            );
            println!(
                "{}",
                serde_json::to_string_pretty(&state.runtime_check_status()?)?
            );
        }
        Commands::CaptureDiagnose { runtime, thread } => {
            let mut client =
                Client::open(&runtime.codex_binary, &runtime.codex_home, &runtime.project).await?;
            let diagnostics = client.diagnose_thread(&thread).await;
            client.close().await?;
            println!("{}", serde_json::to_string_pretty(&diagnostics?)?);
        }
        Commands::Worker { config, once } => {
            coding_memory::worker::run(
                &mut state,
                &root,
                &coding_memory::worker::Config::load(&config)?,
                &std::env::current_exe()?,
                once,
            )
            .await?
        }
        Commands::WorkerStatus => {
            println!("{}", serde_json::to_string_pretty(&state.worker_status()?)?)
        }
        Commands::ServicePlist {
            config,
            output,
            executable,
            label,
        } => {
            let executable = executable.unwrap_or(std::env::current_exe()?);
            let plist = coding_memory::service::plist(&root, &config, &executable, &label)?;
            coding_memory::service::write(&output, &plist)?;
            println!("{}", output.display());
        }
        Commands::Consolidate {
            vault,
            project_id,
            force,
            min_ready,
            max_age_seconds,
            codex_binary,
            model,
            review_model,
            timeout_seconds,
            loopback,
        } => {
            let proposer = CodexExec {
                root: root.clone(),
                binary: codex_binary,
                guard_binary: std::env::current_exe()?,
                model,
                timeout: std::time::Duration::from_secs(timeout_seconds),
                loopback,
            };
            let reviewer = CodexExec {
                model: review_model,
                ..proposer.clone()
            };
            let batch = coding_memory::consolidation_worker::consolidate(
                &mut state,
                &root,
                &vault,
                &project_id,
                coding_memory::consolidation_state::Trigger {
                    force,
                    min_ready,
                    max_age_seconds,
                },
                &proposer,
                &reviewer,
            )
            .await?;
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({"batch":batch}))?
            );
        }
        Commands::ConsolidationStatus => println!(
            "{}",
            serde_json::to_string_pretty(&state.consolidation_status()?)?
        ),
        Commands::ConsolidationRetry { batch } => state.retry_batch(batch, ingestion::now())?,
        Commands::VaultRecover { vault, batch } => println!(
            "{}",
            coding_memory::vault::apply(&mut state, &root, &vault, batch, |_| Ok(()))?
        ),
        Commands::EvidenceCleanup {
            vault,
            retention_days,
            apply,
        } => println!(
            "{}",
            serde_json::to_string_pretty(&coding_memory::retention::cleanup(
                &mut state,
                &root,
                &vault,
                ingestion::now(),
                retention_days,
                apply
            )?)?
        ),
        Commands::GenerationSetup => {
            let backend = CodexExec {
                root: root.clone(),
                binary: "codex".into(),
                guard_binary: std::env::current_exe()?,
                model: "gpt-6.1-sol".into(),
                timeout: std::time::Duration::from_secs(300),
                loopback: None,
            };
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &serde_json::json!({"codex_home":backend.prepare()?,"login":"Independent login required; existing credentials are never copied."})
                )?
            );
        }
        Commands::ExternalPolicy {
            project_id,
            decision,
        } => state.set_external_policy(&project_id, matches!(decision, ExternalDecision::Allow))?,
        Commands::ExtractionStatus { project_id } => println!(
            "{}",
            serde_json::to_string_pretty(&state.extraction_status(&project_id)?)?
        ),
        Commands::Candidates { project_id } => println!(
            "{}",
            serde_json::to_string_pretty(&state.candidates(&project_id)?)?
        ),
        Commands::ExtractionRetry {
            store,
            thread,
            turn,
        } => state.retry_extraction(
            &RunKey {
                store,
                thread,
                turn,
            },
            ingestion::now(),
        )?,
        Commands::Extract {
            project_id,
            codex_binary,
            model,
            timeout_seconds,
            loopback,
        } => {
            let backend = CodexExec {
                root: root.clone(),
                binary: codex_binary,
                guard_binary: std::env::current_exe()?,
                model,
                timeout: std::time::Duration::from_secs(timeout_seconds),
                loopback,
            };
            state.enqueue_extractions(&project_id, ingestion::now())?;
            while extraction_worker::extract_one(&mut state, &root, &project_id, &backend).await? {}
            println!(
                "{}",
                serde_json::to_string_pretty(&state.extraction_status(&project_id)?)?
            );
        }
        Commands::IndexRebuild { vault } => println!(
            "{}",
            serde_json::to_string_pretty(&Search::new(&root, &vault)?.rebuild()?)?
        ),
        Commands::Search {
            vault,
            project_id,
            query,
            limit,
            other_project,
            include_historical,
        } => println!(
            "{}",
            serde_json::to_string_pretty(&Search::new(&root, &vault)?.search(
                &query,
                &Scope {
                    project_id,
                    other_projects: other_project,
                    include_historical
                },
                limit
            )?)?
        ),
        Commands::Read {
            vault,
            project_id,
            id,
            other_project,
            include_historical,
        } => println!(
            "{}",
            serde_json::to_string_pretty(&Search::new(&root, &vault)?.read(
                &id,
                &Scope {
                    project_id,
                    other_projects: other_project,
                    include_historical
                }
            )?)?
        ),
        Commands::Mcp { vault, project_id } => {
            mcp::serve(Search::new(&root, &vault)?, project_id).await?
        }
        Commands::Status => println!("{}", serde_json::to_string_pretty(&state.status()?)?),
        Commands::Evidence { digest } => println!(
            "{}",
            serde_json::to_string_pretty(&evidence::load(&root, &digest)?)?
        ),
        Commands::OwnStore { codex_home } => state.own_store(&ingestion::store_id(&codex_home)?)?,
        Commands::Retry {
            store,
            thread,
            turn,
        } => state.retry(
            &RunKey {
                store,
                thread,
                turn,
            },
            ingestion::now(),
        )?,
        Commands::Enqueue { codex_home, event } => {
            let mut bytes = Vec::new();
            if let Some(event) = event {
                bytes = event.into_bytes();
            } else {
                io::stdin().take(65537).read_to_end(&mut bytes)?;
            }
            ensure!(bytes.len() <= 65536, "Signal exceeds size limit");
            let result = ingestion::enqueue_signal(
                &mut state,
                &codex_home,
                &serde_json::from_slice(&bytes)?,
            )?;
            println!("{result:?}");
        }
        command => {
            let (runtime, operation) = match command {
                Commands::Register(r) => (r, "register"),
                Commands::Reconcile(r) => (r, "reconcile"),
                Commands::Capture(r) => (r, "capture"),
                _ => unreachable!(),
            };
            let mut client =
                Client::open(&runtime.codex_binary, &runtime.codex_home, &runtime.project).await?;
            match operation {
                "register" => println!(
                    "{}",
                    ingestion::register(
                        &mut state,
                        &mut client,
                        &runtime.codex_home,
                        &runtime.project
                    )
                    .await?
                ),
                "reconcile" => println!(
                    "{}",
                    ingestion::reconcile(
                        &mut state,
                        &mut client,
                        &runtime.codex_home,
                        &runtime.project
                    )
                    .await?
                ),
                _ => {
                    while ingestion::capture_one(
                        &mut state,
                        &mut client,
                        &runtime.codex_home,
                        &root,
                    )
                    .await?
                    {}
                }
            }
            client.close().await?;
        }
    }
    Ok(())
}
