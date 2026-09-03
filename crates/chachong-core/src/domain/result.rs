use serde::{Deserialize, Serialize};

use super::{BatchId, FileId, ReferenceLibraryId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextRange {
    /// Inclusive UTF-8 byte offset in the canonical parsed text.
    pub start: u64,
    /// Exclusive UTF-8 byte offset in the canonical parsed text.
    pub end: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RiskRegion {
    pub query_range: TextRange,
    pub source_range: Option<TextRange>,
    pub score: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ComparisonScope {
    WithinBatch { batch_id: BatchId },
    ReferenceLibrary { library_id: ReferenceLibraryId },
}

/// A file-to-file result. Work-item aggregation is intentionally outside this model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileComparison {
    pub query_file_id: FileId,
    pub source_file_id: FileId,
    pub scope: ComparisonScope,
    pub algorithm_id: String,
    pub similarity: f32,
    pub risk_regions: Vec<RiskRegion>,
}
