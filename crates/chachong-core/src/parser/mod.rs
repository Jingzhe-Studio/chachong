mod code;
mod document;
mod libreoffice;

use std::{error::Error, fmt, path::Path, path::PathBuf, sync::Arc};

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

pub async fn parse_in_background(
    parser: Arc<dyn ContentParser>,
    file: ManagedFile,
    path: PathBuf,
) -> Result<ParsedFile, ParseError> {
    tokio::spawn(async move { parser.parse(&file, &path).await })
        .await
        .map_err(|error| ParseError::new(format!("解析器异常终止：{error}")))?
}

pub use code::CodeTextParser;
pub use document::LiteParseDocumentParser;
pub use libreoffice::libreoffice_available;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{FileId, ParseStatus};

    struct PanickingParser;

    #[async_trait]
    impl ContentParser for PanickingParser {
        fn id(&self) -> &'static str {
            "panicking-test-parser"
        }

        fn supports(&self, _file: &ManagedFile) -> bool {
            true
        }

        async fn parse(&self, _file: &ManagedFile, _path: &Path) -> Result<ParsedFile, ParseError> {
            panic!("simulated parser failure")
        }
    }

    #[tokio::test]
    async fn background_parse_turns_panics_into_errors() {
        let file = ManagedFile {
            id: FileId(1),
            name: "fixture.pdf".into(),
            relative_path: "fixture.pdf".into(),
            object_key: "objects/fixture.pdf".into(),
            parsed_key: None,
            content_hash: "fixture".into(),
            extension: "pdf".into(),
            size_bytes: 0,
            category: FileCategory::Document,
            parse_status: ParseStatus::Parsing,
            parse_error: None,
            parser_id: Some("panicking-test-parser".into()),
        };

        let error = parse_in_background(
            Arc::new(PanickingParser),
            file,
            PathBuf::from("fixture.pdf"),
        )
        .await
        .unwrap_err();

        assert!(error.to_string().contains("解析器异常终止"));
    }
}
