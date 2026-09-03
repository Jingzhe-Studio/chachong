use std::{path::Path, path::PathBuf, sync::Arc};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    detection::DetectionError,
    domain::{
        FileCategory, FileId, ManagedFile, ParseStatus, ReferenceLibrary, ReferenceLibraryId,
        ReferenceLibrarySummary,
    },
    importing::{ImportError, ReferenceImportSkip, discover_reference_files},
    parser::{CodeTextParser, ContentParser, LiteParseDocumentParser},
    storage::{
        InsertReferenceFileOutcome, NewReferenceFile, ObjectStore, SqliteStore, StorageError,
        StorageLayout,
    },
};

#[derive(Debug, Error)]
pub enum ApplicationError {
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error("后台任务失败：{0}")]
    BackgroundTask(String),
    #[error(transparent)]
    Import(#[from] ImportError),
    #[error(transparent)]
    Detection(#[from] DetectionError),
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceFileProgress {
    pub library_id: ReferenceLibraryId,
    pub file: ManagedFile,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SkippedReferenceImport {
    pub path: String,
    pub reason: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceImportSummary {
    pub imported: u64,
    pub ready: u64,
    pub failed: u64,
    pub skipped: u64,
    pub skipped_items: Vec<SkippedReferenceImport>,
}

pub struct ReferenceLibraryService {
    store: Arc<SqliteStore>,
    objects: ObjectStore,
    document_parser: Arc<dyn ContentParser>,
    code_parser: Arc<dyn ContentParser>,
}

impl ReferenceLibraryService {
    pub fn open(data_root: impl AsRef<Path>) -> Result<Self, ApplicationError> {
        let layout = StorageLayout::under(data_root);
        let store = Arc::new(SqliteStore::open(&layout)?);
        store.recover_interrupted_parses()?;
        Ok(Self {
            store,
            objects: ObjectStore::new(layout)?,
            document_parser: Arc::new(LiteParseDocumentParser::default()),
            code_parser: Arc::new(CodeTextParser),
        })
    }

    pub fn list_libraries(&self) -> Result<Vec<ReferenceLibrarySummary>, ApplicationError> {
        Ok(self.store.list_reference_libraries()?)
    }

    pub fn create_library(
        &self,
        name: &str,
        category: FileCategory,
    ) -> Result<ReferenceLibrary, ApplicationError> {
        Ok(self.store.create_reference_library(name, category)?)
    }

    pub fn rename_library(
        &self,
        id: ReferenceLibraryId,
        name: &str,
    ) -> Result<ReferenceLibrary, ApplicationError> {
        Ok(self.store.rename_reference_library(id, name)?)
    }

    pub fn delete_library(&self, id: ReferenceLibraryId) -> Result<(), ApplicationError> {
        let cleanup = self.store.delete_reference_library(id)?;
        self.objects.remove_keys(cleanup.all());
        self.store.invalidate_all_detections()?;
        Ok(())
    }

    pub fn list_files(
        &self,
        library_id: ReferenceLibraryId,
    ) -> Result<Vec<ManagedFile>, ApplicationError> {
        Ok(self.store.list_reference_files(library_id)?)
    }

    pub fn delete_file(
        &self,
        library_id: ReferenceLibraryId,
        file_id: FileId,
    ) -> Result<(), ApplicationError> {
        let cleanup = self.store.delete_reference_file(library_id, file_id)?;
        self.objects.remove_keys(cleanup.all());
        self.store.invalidate_all_detections()?;
        Ok(())
    }

    pub async fn retry_file(
        &self,
        library_id: ReferenceLibraryId,
        file_id: FileId,
    ) -> Result<ManagedFile, ApplicationError> {
        let file = self
            .store
            .find_reference_file(library_id, file_id)?
            .ok_or_else(|| StorageError::Validation("参考文件不存在".into()))?;
        let file = self.parse_file(library_id, file, &mut |_| {}).await?;
        if file.parse_status == ParseStatus::Ready {
            self.store.invalidate_all_detections()?;
        }
        Ok(file)
    }

    pub async fn import_paths<F>(
        &self,
        library_id: ReferenceLibraryId,
        paths: Vec<PathBuf>,
        mut progress: F,
    ) -> Result<ReferenceImportSummary, ApplicationError>
    where
        F: FnMut(&ReferenceFileProgress) + Send,
    {
        let library = self
            .store
            .find_reference_library(library_id)?
            .ok_or_else(|| StorageError::Validation("参考库不存在".into()))?;
        let category = library.category;
        let discovery =
            tokio::task::spawn_blocking(move || discover_reference_files(category, &paths))
                .await
                .map_err(|error| ApplicationError::BackgroundTask(error.to_string()))?;

        let mut summary = ReferenceImportSummary {
            skipped: discovery.skipped.len() as u64,
            skipped_items: discovery.skipped.into_iter().map(Into::into).collect(),
            ..ReferenceImportSummary::default()
        };

        for candidate in discovery.candidates {
            let object_store = self.objects.clone();
            let source_path = candidate.source_path.clone();
            let object =
                match tokio::task::spawn_blocking(move || object_store.import(&source_path))
                    .await
                    .map_err(|error| ApplicationError::BackgroundTask(error.to_string()))?
                {
                    Ok(object) => object,
                    Err(error) => {
                        summary.skipped += 1;
                        summary.skipped_items.push(SkippedReferenceImport {
                            path: candidate.source_path.to_string_lossy().into_owned(),
                            reason: error.to_string(),
                        });
                        continue;
                    }
                };

            let parser = self.parser_for(category);
            let parsed_key = self.objects.parsed_key(parser.id(), &object.content_hash);
            let reusable_parsed_key = self
                .objects
                .parsed_exists(&parsed_key)
                .then_some(parsed_key);
            let outcome = self.store.insert_reference_file(NewReferenceFile {
                library_id,
                name: candidate.name,
                relative_path: candidate.relative_path,
                category,
                object,
                parser_id: parser.id().into(),
                reusable_parsed_key,
            })?;

            let file = match outcome {
                InsertReferenceFileOutcome::Duplicate(file) => {
                    summary.skipped += 1;
                    summary.skipped_items.push(SkippedReferenceImport {
                        path: candidate.source_path.to_string_lossy().into_owned(),
                        reason: "参考库中已存在相同内容".into(),
                    });
                    progress(&ReferenceFileProgress { library_id, file });
                    continue;
                }
                InsertReferenceFileOutcome::Inserted(file) => file,
            };

            summary.imported += 1;
            progress(&ReferenceFileProgress {
                library_id,
                file: file.clone(),
            });
            let file = if file.parse_status == ParseStatus::Ready {
                file
            } else {
                self.parse_file(library_id, file, &mut progress).await?
            };
            match file.parse_status {
                ParseStatus::Ready => summary.ready += 1,
                ParseStatus::Failed => summary.failed += 1,
                ParseStatus::Pending | ParseStatus::Parsing => {}
            }
        }

        if summary.ready > 0 {
            self.store.invalidate_all_detections()?;
        }
        Ok(summary)
    }

    fn parser_for(&self, category: FileCategory) -> &dyn ContentParser {
        match category {
            FileCategory::Document => self.document_parser.as_ref(),
            FileCategory::Code => self.code_parser.as_ref(),
        }
    }

    async fn parse_file<F>(
        &self,
        library_id: ReferenceLibraryId,
        file: ManagedFile,
        progress: &mut F,
    ) -> Result<ManagedFile, ApplicationError>
    where
        F: FnMut(&ReferenceFileProgress) + Send,
    {
        let parser = self.parser_for(file.category);
        let parsing = self.store.update_parse_status(
            file.id,
            ParseStatus::Parsing,
            parser.id(),
            None,
            None,
        )?;
        progress(&ReferenceFileProgress {
            library_id,
            file: parsing.clone(),
        });

        let source = self.objects.object_path(&file.object_key);
        let parsed_key = self.objects.parsed_key(parser.id(), &file.content_hash);
        let parse_result = parser.parse(&parsing, &source).await;
        let final_file = match parse_result {
            Ok(parsed) => match self.objects.write_parsed(&parsed_key, &parsed).await {
                Ok(()) => self.store.update_parse_status(
                    file.id,
                    ParseStatus::Ready,
                    parser.id(),
                    Some(&parsed_key),
                    None,
                )?,
                Err(error) => self.store.update_parse_status(
                    file.id,
                    ParseStatus::Failed,
                    parser.id(),
                    None,
                    Some(&format!("保存解析结果失败：{error}")),
                )?,
            },
            Err(error) => self.store.update_parse_status(
                file.id,
                ParseStatus::Failed,
                parser.id(),
                None,
                Some(&error.to_string()),
            )?,
        };
        progress(&ReferenceFileProgress {
            library_id,
            file: final_file.clone(),
        });
        Ok(final_file)
    }
}

impl From<ReferenceImportSkip> for SkippedReferenceImport {
    fn from(value: ReferenceImportSkip) -> Self {
        Self {
            path: value.path,
            reason: value.reason,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    #[tokio::test]
    async fn imports_code_and_persists_ready_status() {
        let directory = tempdir().unwrap();
        let source = directory.path().join("main.rs");
        fs::write(&source, "fn main() {}\n").unwrap();
        let service = ReferenceLibraryService::open(directory.path().join("data")).unwrap();
        let library = service.create_library("Rust", FileCategory::Code).unwrap();

        let summary = service
            .import_paths(library.id, vec![source], |_| {})
            .await
            .unwrap();

        assert_eq!(summary.ready, 1);
        assert_eq!(
            service.list_files(library.id).unwrap()[0].parse_status,
            ParseStatus::Ready
        );
    }

    #[tokio::test]
    async fn reuses_managed_content_and_parse_artifact_across_libraries() {
        let directory = tempdir().unwrap();
        let source = directory.path().join("shared.rs");
        fs::write(&source, "pub fn shared() -> bool { true }\n").unwrap();
        let data_root = directory.path().join("data");
        let service = ReferenceLibraryService::open(&data_root).unwrap();
        let first = service
            .create_library("课程 A", FileCategory::Code)
            .unwrap();
        let second = service
            .create_library("课程 B", FileCategory::Code)
            .unwrap();

        service
            .import_paths(first.id, vec![source.clone()], |_| {})
            .await
            .unwrap();
        service
            .import_paths(second.id, vec![source], |_| {})
            .await
            .unwrap();

        let first_file = service.list_files(first.id).unwrap().remove(0);
        let second_file = service.list_files(second.id).unwrap().remove(0);
        assert_eq!(first_file.object_key, second_file.object_key);
        assert_eq!(first_file.parsed_key, second_file.parsed_key);
        assert_eq!(second_file.parse_status, ParseStatus::Ready);

        let object_path = data_root.join(&first_file.object_key);
        let parsed_path = data_root.join(first_file.parsed_key.as_ref().unwrap());
        service.delete_library(first.id).unwrap();
        assert!(object_path.exists());
        assert!(parsed_path.exists());
        service.delete_library(second.id).unwrap();
        assert!(!object_path.exists());
        assert!(!parsed_path.exists());
    }

    #[tokio::test]
    async fn keeps_failed_document_and_allows_retry() {
        let directory = tempdir().unwrap();
        let source = directory.path().join("broken.pdf");
        fs::write(&source, b"not a PDF").unwrap();
        let service = ReferenceLibraryService::open(directory.path().join("data")).unwrap();
        let library = service
            .create_library("损坏样本", FileCategory::Document)
            .unwrap();

        let summary = service
            .import_paths(library.id, vec![source], |_| {})
            .await
            .unwrap();
        assert_eq!(summary.failed, 1);
        let failed = service.list_files(library.id).unwrap().remove(0);
        assert_eq!(failed.parse_status, ParseStatus::Failed);
        assert!(failed.parse_error.as_deref().unwrap().contains("LiteParse"));

        let retried = service.retry_file(library.id, failed.id).await.unwrap();
        assert_eq!(retried.parse_status, ParseStatus::Failed);
        assert!(retried.parse_error.is_some());
    }
}
