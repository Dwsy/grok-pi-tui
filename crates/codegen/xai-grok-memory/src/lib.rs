//! Cross-session memory for Grok.
//!
//! Memory files are markdown under `~/.grok/memory/`, one global file plus a subdirectory per workspace.
//!
//! ## Data Layout
//!
//! ```text
//! ~/.grok/memory/
//!   ├── MEMORY.md                         # Global curated knowledge
//!   └── {workspace_hash}/                 # Per-workspace (blake3(cwd)[..16])
//!       ├── MEMORY.md                     # Project-level curated knowledge
//!       └── sessions/
//!           └── YYYY-MM-DD-{slug}-{sid8}.md  # Session logs
//! ```
//!
//! ## Feature Flag
//!
//! Memory is enabled through `GROK_MEMORY`, `[memory] enabled`, or remote settings.
//! When disabled, this crate is not initialized by the host.

pub mod archive;
pub mod backend;
pub mod chunker;
pub mod dream;
pub mod dream_lock;
pub mod embedding;
pub mod flush;
pub mod index;
pub mod mmr;
pub mod observation;
pub mod query_expansion;
pub mod schema;
pub mod search;
pub mod storage;
mod storage_v2;
pub mod text_utils;
pub mod v2;
mod v2_access;
pub mod v2_capture;
pub mod watcher;

pub use backend::{EndpointScopedCredentials, MemoryBackendImpl, MemoryBackendParams};
pub use index::{MemoryIndex, init_sqlite_vec};
pub use observation::*;
pub use storage::{MemoryScope, MemoryStorage, SaveRememberNoteError};
pub use v2::{
    MAX_MANUAL_OBSERVATION_BYTES, V2Manifest, V2ManifestBudget, V2MemoryScope, V2StorageError,
    ensure_scope_initialized, regenerate_scope_manifest, render_scope_manifest,
};
pub use v2_access::{V2AccessError, V2MemoryAccessPolicy, V2PathClass};
pub use v2_capture::{
    CaptureCursors, CaptureJob, CaptureLease, CaptureOutcomeDraft, CaptureRange, ClaimRequest,
    CommitResult, ObservationDraft, ObservationType, V2CaptureError, V2CaptureStore,
};

pub(crate) const MEMORY_LOG_TARGET: &str = "xai_memory";

/// Embed all chunks that don't have embeddings yet.
/// Call after reindex, flush writes, or session-end writes.
pub async fn embed_missing_chunks(
    index: &MemoryIndex,
    provider: &dyn embedding::EmbeddingProvider,
) -> usize {
    let chunks = match index.chunks_without_embeddings() {
        Ok(c) if c.is_empty() => return 0,
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(
                target: MEMORY_LOG_TARGET,
                error = %e,
                "failed to query chunks without embeddings"
            );
            return 0;
        }
    };

    let total = chunks.len();
    let mut embedded = 0;

    // 32 is the provider's typical max batch size
    for batch in chunks.chunks(32) {
        let texts: Vec<&str> = batch.iter().map(|(_, text)| text.as_str()).collect();
        match provider.embed_batch(&texts).await {
            Ok(embeddings) => {
                for ((chunk_id, _), embedding) in batch.iter().zip(embeddings.iter()) {
                    if let Err(e) = index.upsert_embedding(chunk_id, embedding) {
                        tracing::warn!(
                            target: MEMORY_LOG_TARGET,
                            chunk_id,
                            error = %e,
                            "failed to upsert embedding"
                        );
                    } else {
                        embedded += 1;
                    }
                }
            }
            Err(e) => {
                tracing::warn!(
                    target: MEMORY_LOG_TARGET,
                    error = %e,
                    batch_size = texts.len(),
                    "embedding batch failed, skipping"
                );
            }
        }
    }

    if embedded > 0 {
        tracing::info!(
            target: MEMORY_LOG_TARGET,
            embedded,
            total,
            "embedded missing chunks"
        );
    }
    embedded
}
