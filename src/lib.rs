//! Durable ingestion and evidence capture for coding-agent memory.
pub mod codex;
pub mod consolidation_state;
pub mod domain;
pub mod evidence;
pub mod extraction;
pub mod extraction_state;
pub mod extraction_worker;
pub mod generation;
pub mod ingestion;
pub mod mcp;
pub mod notes;
pub mod project;
pub mod search;
pub mod state;

pub mod vault;

pub mod consolidation;

pub mod consolidation_retrieval;
pub mod consolidation_worker;

pub mod retention;

pub mod worker;

pub mod service;
