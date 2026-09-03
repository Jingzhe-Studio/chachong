use std::path::Path;

use async_trait::async_trait;
use liteparse::{LiteParse, LiteParseConfig, OutputFormat, config::ImageMode};

use crate::domain::{FileCategory, ManagedFile, TextRange};

use super::{
    ContentParser, LocationMapping, ParseError, ParsedFile, SourceLocation, libreoffice_available,
};

pub const LITEPARSE_PARSER_ID: &str = "liteparse-v2";

pub struct LiteParseDocumentParser {
    parser: LiteParse,
}

impl Default for LiteParseDocumentParser {
    fn default() -> Self {
        let config = LiteParseConfig {
            ocr_enabled: false,
            output_format: OutputFormat::Text,
            image_mode: ImageMode::Off,
            extract_links: false,
            quiet: true,
            continue_on_page_error: false,
            ..LiteParseConfig::default()
        };

        Self {
            parser: LiteParse::new(config),
        }
    }
}

#[async_trait]
impl ContentParser for LiteParseDocumentParser {
    fn id(&self) -> &'static str {
        LITEPARSE_PARSER_ID
    }

    fn supports(&self, file: &ManagedFile) -> bool {
        file.category == FileCategory::Document
            && matches!(file.extension.as_str(), "pdf" | "doc" | "docx")
    }

    async fn parse(&self, file: &ManagedFile, path: &Path) -> Result<ParsedFile, ParseError> {
        validate_office_dependency(&file.extension, libreoffice_available())?;

        let path = path
            .to_str()
            .ok_or_else(|| ParseError::new("文件路径不是有效的 UTF-8"))?;
        let result = self
            .parser
            .parse(path)
            .await
            .map_err(|error| ParseError::new(format!("LiteParse 解析失败：{error}")))?;

        let mut text = String::new();
        let mut locations = Vec::new();

        for page in result.pages {
            if !text.is_empty() {
                text.push_str("\n\n");
            }

            let start = text.len() as u64;
            text.push_str(&page.text);
            let end = text.len() as u64;

            if start != end {
                locations.push(LocationMapping {
                    text_range: TextRange { start, end },
                    source: SourceLocation::Document {
                        page: Some(page.page_number as u32),
                        block: None,
                    },
                });
            }
        }

        if text.trim().is_empty() {
            return Err(ParseError::new(
                "未提取到文本；如果这是扫描件，当前版本尚未启用 OCR",
            ));
        }

        Ok(ParsedFile {
            file_id: file.id,
            category: FileCategory::Document,
            text,
            locations,
        })
    }
}

fn validate_office_dependency(extension: &str, available: bool) -> Result<(), ParseError> {
    if matches!(extension, "doc" | "docx") && !available {
        return Err(ParseError::new(
            "解析 Word 文件需要安装 LibreOffice，并将 soffice 加入 PATH",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use lopdf::{
        Document, Object, Stream,
        content::{Content, Operation},
        dictionary,
    };
    use tempfile::tempdir;

    use crate::domain::{FileId, ParseStatus};

    use super::*;

    fn managed_document(extension: &str) -> ManagedFile {
        ManagedFile {
            id: FileId(1),
            name: format!("fixture.{extension}"),
            relative_path: format!("fixture.{extension}"),
            object_key: format!("objects/fixture.{extension}"),
            parsed_key: None,
            content_hash: "fixture".into(),
            extension: extension.into(),
            size_bytes: 0,
            category: FileCategory::Document,
            parse_status: ParseStatus::Parsing,
            parse_error: None,
            parser_id: Some(LITEPARSE_PARSER_ID.into()),
        }
    }

    fn write_pdf(path: &Path, page_texts: &[Option<&str>]) {
        let mut document = Document::with_version("1.5");
        let pages_id = document.new_object_id();
        let font_id = document.add_object(dictionary! {
            "Type" => "Font",
            "Subtype" => "Type1",
            "BaseFont" => "Helvetica",
        });
        let resources_id = document.add_object(dictionary! {
            "Font" => dictionary! { "F1" => font_id },
        });
        let mut page_ids = Vec::new();

        for text in page_texts {
            let operations = text.map_or_else(Vec::new, |text| {
                vec![
                    Operation::new("BT", vec![]),
                    Operation::new("Tf", vec!["F1".into(), 14.into()]),
                    Operation::new("Td", vec![72.into(), 720.into()]),
                    Operation::new("Tj", vec![Object::string_literal(text)]),
                    Operation::new("ET", vec![]),
                ]
            });
            let content = Content { operations };
            let content_id = document.add_object(Stream::new(
                dictionary! {},
                content.encode().expect("encode PDF content"),
            ));
            let page_id = document.add_object(dictionary! {
                "Type" => "Page",
                "Parent" => pages_id,
                "Contents" => content_id,
            });
            page_ids.push(page_id.into());
        }

        document.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => page_ids,
                "Count" => page_texts.len() as i64,
                "Resources" => resources_id,
                "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
            }),
        );
        let catalog_id = document.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
        });
        document.trailer.set("Root", catalog_id);
        document.compress();
        document.save(path).expect("save PDF fixture");
    }

    #[test]
    fn reports_missing_libreoffice_for_word() {
        let error = validate_office_dependency("docx", false).unwrap_err();
        assert!(error.to_string().contains("LibreOffice"));
        assert!(validate_office_dependency("pdf", false).is_ok());
    }

    #[tokio::test]
    async fn parses_pdf_and_records_utf8_page_ranges() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("two-pages.pdf");
        write_pdf(&path, &[Some("first page"), Some("second page")]);

        let parsed = LiteParseDocumentParser::default()
            .parse(&managed_document("pdf"), &path)
            .await
            .unwrap();

        assert!(parsed.text.contains("first page"));
        assert!(parsed.text.contains("second page"));
        assert_eq!(parsed.locations.len(), 2);
        for location in &parsed.locations {
            let range = location.text_range.start as usize..location.text_range.end as usize;
            assert!(
                parsed.text.get(range).is_some(),
                "range must follow UTF-8 byte offsets"
            );
        }
        assert!(matches!(
            parsed.locations[0].source,
            SourceLocation::Document { page: Some(1), .. }
        ));
        assert!(matches!(
            parsed.locations[1].source,
            SourceLocation::Document { page: Some(2), .. }
        ));
    }

    #[tokio::test]
    async fn rejects_empty_pdf_with_ocr_explanation() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("blank.pdf");
        write_pdf(&path, &[None]);

        let error = LiteParseDocumentParser::default()
            .parse(&managed_document("pdf"), &path)
            .await
            .unwrap_err();

        assert!(error.to_string().contains("OCR"));
    }

    #[tokio::test]
    async fn rejects_corrupt_pdf() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("broken.pdf");
        fs::write(&path, b"not a PDF").unwrap();

        let error = LiteParseDocumentParser::default()
            .parse(&managed_document("pdf"), &path)
            .await
            .unwrap_err();

        assert!(error.to_string().contains("LiteParse"));
    }
}
