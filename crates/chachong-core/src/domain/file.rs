use serde::{Deserialize, Serialize};

use super::FileId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FileCategory {
    Document,
    Code,
}

impl FileCategory {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Document => "document",
            Self::Code => "code",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "document" => Some(Self::Document),
            "code" => Some(Self::Code),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ParseStatus {
    Pending,
    Parsing,
    Ready,
    Failed,
}

impl ParseStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Parsing => "parsing",
            Self::Ready => "ready",
            Self::Failed => "failed",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(Self::Pending),
            "parsing" => Some(Self::Parsing),
            "ready" => Some(Self::Ready),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedFile {
    pub id: FileId,
    pub name: String,
    pub relative_path: String,
    pub object_key: String,
    pub parsed_key: Option<String>,
    pub content_hash: String,
    pub extension: String,
    pub size_bytes: u64,
    pub category: FileCategory,
    pub parse_status: ParseStatus,
    pub parse_error: Option<String>,
    pub parser_id: Option<String>,
}
