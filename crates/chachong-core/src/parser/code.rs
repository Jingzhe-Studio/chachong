use std::path::Path;

use async_trait::async_trait;

use crate::domain::{FileCategory, ManagedFile, TextRange};

use super::{ContentParser, LocationMapping, ParseError, ParsedFile, SourceLocation};

pub const CODE_TEXT_PARSER_ID: &str = "code-text-v1";

#[derive(Default)]
pub struct CodeTextParser;

#[async_trait]
impl ContentParser for CodeTextParser {
    fn id(&self) -> &'static str {
        CODE_TEXT_PARSER_ID
    }

    fn supports(&self, file: &ManagedFile) -> bool {
        file.category == FileCategory::Code
    }

    async fn parse(&self, file: &ManagedFile, path: &Path) -> Result<ParsedFile, ParseError> {
        let bytes = tokio::fs::read(path)
            .await
            .map_err(|error| ParseError::new(format!("读取代码文件失败：{error}")))?;

        if bytes.iter().take(8192).any(|byte| *byte == 0) {
            return Err(ParseError::new("文件包含二进制内容"));
        }

        let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&bytes);
        let text = String::from_utf8(bytes.to_vec())
            .map_err(|_| ParseError::new("代码文件不是有效的 UTF-8 编码"))?;

        if text.is_empty() {
            return Err(ParseError::new("代码文件为空"));
        }

        let mut offset = 0_u64;
        let mut locations = Vec::new();
        for (index, line) in text.split_inclusive('\n').enumerate() {
            let end = offset + line.len() as u64;
            locations.push(LocationMapping {
                text_range: TextRange { start: offset, end },
                source: SourceLocation::Code {
                    line: index as u32 + 1,
                    column: 1,
                },
            });
            offset = end;
        }

        Ok(ParsedFile {
            file_id: file.id,
            category: FileCategory::Code,
            text,
            locations,
        })
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;
    use crate::domain::{FileId, ParseStatus};

    fn managed_file() -> ManagedFile {
        ManagedFile {
            id: FileId(1),
            name: "main.rs".into(),
            relative_path: "main.rs".into(),
            object_key: "objects/00/file.rs".into(),
            parsed_key: None,
            content_hash: "hash".into(),
            extension: "rs".into(),
            size_bytes: 12,
            category: FileCategory::Code,
            parse_status: ParseStatus::Pending,
            parse_error: None,
            parser_id: None,
        }
    }

    #[tokio::test]
    async fn parses_utf8_code_and_maps_lines() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("main.rs");
        tokio::fs::write(&path, "fn main() {}\n").await.unwrap();

        let parsed = CodeTextParser.parse(&managed_file(), &path).await.unwrap();

        assert_eq!(parsed.text, "fn main() {}\n");
        assert_eq!(parsed.locations.len(), 1);
    }

    #[tokio::test]
    async fn rejects_non_utf8_code() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("main.rs");
        tokio::fs::write(&path, [0xff, 0xfe, 0xfd]).await.unwrap();

        let error = CodeTextParser
            .parse(&managed_file(), &path)
            .await
            .unwrap_err();

        assert!(error.to_string().contains("UTF-8"));
    }
}
