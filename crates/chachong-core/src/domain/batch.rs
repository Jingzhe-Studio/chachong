use serde::{Deserialize, Serialize};

use super::{BatchId, FileId, WorkItemId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DetectionStatus {
    Pending,
    Running,
    Ready,
    Failed,
}

impl DetectionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Ready => "ready",
            Self::Failed => "failed",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(Self::Pending),
            "running" => Some(Self::Running),
            "ready" => Some(Self::Ready),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Batch {
    pub id: BatchId,
    pub name: String,
    pub work_item_ids: Vec<WorkItemId>,
}

/// A material package inside a batch. It is not a user or student account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkItem {
    pub id: WorkItemId,
    pub batch_id: BatchId,
    pub name: String,
    pub file_ids: Vec<FileId>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchSummary {
    pub id: BatchId,
    pub name: String,
    pub work_item_count: u64,
    pub file_count: u64,
    pub ready_file_count: u64,
    pub detection_status: DetectionStatus,
    pub algorithm_id: Option<String>,
    pub detection_error: Option<String>,
    pub highest_file_similarity: Option<f32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkItemSummary {
    pub id: WorkItemId,
    pub batch_id: BatchId,
    pub name: String,
    pub file_count: u64,
    pub ready_file_count: u64,
}
