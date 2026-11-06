//! YourHarness's core: everything that isn't drawing. Storage (`harness.sqlite`, the same format
//! the Swift app wrote), Claude Code's session files, the `claude` process and its stream-json,
//! markdown parsing. No GPUI here, so it's testable on its own and the app stays thin over it.

pub mod attachments;
pub mod claude;
pub mod db;
pub mod files;
pub mod schema;
pub mod history;
pub mod library;
pub mod recovery;
pub mod titles;
pub mod markdown;
pub mod mcp;
pub mod memory;
pub mod memory_tools;
pub mod plan;
