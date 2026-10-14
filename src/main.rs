use anyhow::{Result, ensure};
use clap::{Parser, Subcommand};
use coding_memory::{codex::Client, domain::RunKey, evidence, ingestion, state::State};
use coding_memory::{
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
}
#[derive(Subcommand)]
enum Commands {
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
    let root = evidence::private_directory(&cli.state_dir)?;
    let mut state = State::open(&root.join("state.sqlite"))?;
    match cli.command {
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
