mod code;
mod document;
mod libreoffice;

use std::{error::Error, fmt, path::Path};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::domain::{FileCategory, FileId, ManagedFile, TextRange};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SourceLocation {
    Document {
        page: Option<u32>,
        block: Option<u32>,
    },
    Code {
        line: u32,
        column: u32,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocationMapping {
    pub text_range: TextRange,
    pub source: SourceLocation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParsedFile {
    pub file_id: FileId,
    pub category: FileCategory,
    pub text: String,
    pub locations: Vec<LocationMapping>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    message: String,
}

impl ParseError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for ParseError {}

#[async_trait]
pub trait ContentParser: Send + Sync {
    fn id(&self) -> &'static str;
    fn supports(&self, file: &ManagedFile) -> bool;
    async fn parse(&self, file: &ManagedFile, path: &Path) -> Result<ParsedFile, ParseError>;
}

pub use code::CodeTextParser;
pub use document::LiteParseDocumentParser;
pub use libreoffice::libreoffice_available;
