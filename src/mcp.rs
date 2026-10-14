//! Read-only MCP tools backed by the same service as the CLI.
use crate::search::{Scope, Search};
use rmcp::{
    ServerHandler, ServiceExt,
    handler::server::wrapper::Parameters,
    model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerConfig},
    schemars, tool, tool_handler, tool_router,
};
use serde::Deserialize;

fn default_limit() -> usize {
    5
}
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SearchArgs {
    /// Literal English terms, code identifiers, or paths. FTS operators are not accepted as syntax.
    pub query: String,
    #[serde(default = "default_limit")]
    #[schemars(range(min = 1, max = 10))]
    pub limit: usize,
    #[serde(default)]
    pub other_projects: Vec<String>,
    #[serde(default)]
    pub include_historical: bool,
}
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReadArgs {
    pub id: String,
    #[serde(default)]
    pub other_projects: Vec<String>,
    #[serde(default)]
    pub include_historical: bool,
}
#[derive(Clone)]
pub struct MemoryServer {
    search: Search,
    project: String,
}
impl MemoryServer {
    pub fn new(search: Search, project: String) -> anyhow::Result<Self> {
        crate::notes::identity(&project)?;
        Ok(Self { search, project })
    }
    fn scope(&self, other_projects: Vec<String>, include_historical: bool) -> Scope {
        Scope {
            project_id: self.project.clone(),
            other_projects,
            include_historical,
        }
    }
}
fn result<T: serde::Serialize>(value: anyhow::Result<T>) -> CallToolResult {
    match value.and_then(|v| Ok(serde_json::to_value(v)?)) {
        Ok(value) => {
            let mut result = CallToolResult::success(vec![ContentBlock::text(value.to_string())]);
            result.structured_content = Some(value);
            result
        }
        Err(error) => CallToolResult::error(vec![ContentBlock::text(error.to_string())]),
    }
}
#[tool_router]
impl MemoryServer {
    #[tool(
        description = "Search applicable memory summaries for the configured project and global scope. Use English literal terms, identifiers, or paths. Other projects and superseded notes require explicit options.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    fn memory_search(&self, Parameters(args): Parameters<SearchArgs>) -> CallToolResult {
        result(self.search.search(
            &args.query,
            &self.scope(args.other_projects, args.include_historical),
            args.limit,
        ))
    }
    #[tool(
        description = "Read an authoritative memory note selected by stable ID, including applicability, support, review, revision, and local evidence availability. Pending or stale notes are unavailable.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    fn memory_read(&self, Parameters(args): Parameters<ReadArgs>) -> CallToolResult {
        result(self.search.read(
            &args.id,
            &self.scope(args.other_projects, args.include_historical),
        ))
    }
}
#[tool_handler]
impl ServerHandler for MemoryServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(
                "coding-memory",
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(include_str!("../docs/agent-memory-instructions.md"))
    }
}
pub async fn serve(search: Search, project: String) -> anyhow::Result<()> {
    let server = MemoryServer::new(search, project)?
        .serve(rmcp::transport::stdio())
        .await?;
    server.waiting().await?;
    Ok(())
}
