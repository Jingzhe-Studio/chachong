use std::{error::Error, fmt};

use serde::Serialize;

use crate::domain::{FileCategory, FileId, RiskRegion};

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AlgorithmDescriptor {
    pub id: &'static str,
    pub display_name: &'static str,
    pub description: &'static str,
    pub supported_categories: &'static [FileCategory],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedFile {
    pub file_id: FileId,
    pub format_version: u32,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Candidate {
    pub file_id: FileId,
    pub relevance: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ComparisonEvidence {
    pub similarity: f32,
    pub risk_regions: Vec<RiskRegion>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectionError {
    message: String,
}

impl DetectionError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for DetectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for DetectionError {}
