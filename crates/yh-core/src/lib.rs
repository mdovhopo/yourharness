//! YourHarness's core: everything that isn't drawing. Storage (`harness.sqlite`, the same format
//! 0.0.x wrote), Claude Code's session files, the `claude` process and its stream-json,
//! markdown parsing. No GPUI here, so it's testable on its own and the app stays thin over it.

pub mod attachments;
pub mod agents;
pub mod claude;
pub mod dev_servers;
pub mod db;
pub mod diagrams;
pub mod files;
pub mod find;
pub mod footprint;
pub mod forge;
pub mod git;
pub mod github;
pub mod gitlab;
pub mod stacks;
pub mod workspace;
pub mod schema;
pub mod history;
pub mod legacy_import;
pub mod library;
pub mod recovery;
pub mod shell_env;
pub mod subagent_record;
pub mod turn_changes;
pub mod titles;
pub mod markdown;
pub mod links;
pub mod clock;
pub mod limits;
pub mod mcp;
pub mod memory;
pub mod memory_archive;
pub mod memory_tools;
pub mod plan;
pub mod usage;
pub mod state_archive;
pub mod updates;
pub mod cookies;
pub mod memory_stats;
pub mod memory_agents;
