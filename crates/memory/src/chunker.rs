//! Text chunking for memory — re-exports from antre-splitter.

// Re-export the public API so existing consumers of `antre_memory::chunker` continue to work.
pub use antre_splitter::{Chunk, chunk_content, chunk_markdown};
