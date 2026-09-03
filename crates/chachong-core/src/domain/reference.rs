use serde::{Deserialize, Serialize};

use super::{FileCategory, FileId, ReferenceLibraryId};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceLibrary {
    pub id: ReferenceLibraryId,
    pub name: String,
    pub category: FileCategory,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceEntry {
    pub library_id: ReferenceLibraryId,
    pub file_id: FileId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceLibrarySummary {
    pub library: ReferenceLibrary,
    pub file_count: u64,
}
