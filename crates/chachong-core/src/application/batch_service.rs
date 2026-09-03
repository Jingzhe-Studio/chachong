use std::{collections::HashMap, path::Path, path::PathBuf, sync::Arc};

use serde::{Deserialize, Serialize};

use crate::{
    detection::{DetectionAlgorithm, DetectionPipeline},
    domain::{
        BatchId, BatchSummary, ComparisonScope, DetectionStatus, FileCategory, FileId, ManagedFile,
        ParseStatus, RiskRegion, TextRange, WorkItemId, WorkItemSummary,
    },
    importing::discover_batch,
    parser::{CodeTextParser, ContentParser, LiteParseDocumentParser, ParsedFile, SourceLocation},
    storage::{
        InsertBatchFileOutcome, NewBatchFile, NewComparison, ObjectStore, SqliteStore,
        StorageError, StorageLayout, StoredMatchSummary,
    },
};

use super::{ApplicationError, SkippedReferenceImport};

const RETRIEVAL_LIMIT: usize = 64;
const MIN_SIMILARITY: f32 = 0.15;
const MIN_MATCHED_BYTES: u64 = 16;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchFileProgress {
    pub batch_id: BatchId,
    pub work_item_id: WorkItemId,
    pub file: ManagedFile,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchImportSummary {
    pub batch: BatchSummary,
    pub imported: u64,
    pub ready: u64,
    pub failed: u64,
    pub skipped: u64,
    pub skipped_items: Vec<SkippedReferenceImport>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DetectionRunSummary {
    pub batch_id: BatchId,
    pub algorithm_id: String,
    pub query_files: u64,
    pub compared_pairs: u64,
    pub matches: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DetectionProgress {
    pub batch_id: BatchId,
    pub processed_files: u64,
    pub total_files: u64,
    pub matches: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkItemFileView {
    pub file: ManagedFile,
    pub match_count: u64,
    pub highest_similarity: Option<f32>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileMatchSummary {
    pub id: u64,
    pub source_file_id: FileId,
    pub source_name: String,
    pub source_path: String,
    pub source_kind: &'static str,
    pub source_container: String,
    pub source_work_item_name: Option<String>,
    pub algorithm_id: String,
    pub similarity: f32,
    pub risk_count: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MatchDetail {
    pub result_id: u64,
    pub query_file: ManagedFile,
    pub source_file: ManagedFile,
    pub query_text: String,
    pub source_text: String,
    pub similarity: f32,
    pub algorithm_id: String,
    pub risk_regions: Vec<LocatedRiskRegion>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum RiskLocation {
    Document { start_page: u32, end_page: u32 },
    Code { start_line: u32, end_line: u32 },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocatedRiskRegion {
    pub query_range: TextRange,
    pub source_range: Option<TextRange>,
    pub score: f32,
    pub query_location: Option<RiskLocation>,
    pub source_location: Option<RiskLocation>,
}

pub struct BatchService {
    store: Arc<SqliteStore>,
    objects: ObjectStore,
    document_parser: Arc<dyn ContentParser>,
    code_parser: Arc<dyn ContentParser>,
}

impl BatchService {
    pub fn open(data_root: impl AsRef<Path>) -> Result<Self, ApplicationError> {
        let layout = StorageLayout::under(data_root);
        let store = Arc::new(SqliteStore::open(&layout)?);
        store.recover_interrupted_parses()?;
        store.recover_interrupted_detections()?;
        Ok(Self {
            store,
            objects: ObjectStore::new(layout)?,
            document_parser: Arc::new(LiteParseDocumentParser::default()),
            code_parser: Arc::new(CodeTextParser),
        })
    }

    pub fn list_batches(&self) -> Result<Vec<BatchSummary>, ApplicationError> {
        Ok(self.store.list_batches()?)
    }

    pub fn rename_batch(
        &self,
        batch_id: BatchId,
        name: &str,
    ) -> Result<BatchSummary, ApplicationError> {
        Ok(self.store.rename_batch(batch_id, name)?)
    }

    pub fn delete_batch(&self, batch_id: BatchId) -> Result<(), ApplicationError> {
        let cleanup = self.store.delete_batch(batch_id)?;
        self.objects.remove_keys(cleanup.all());
        Ok(())
    }

    pub fn list_work_items(
        &self,
        batch_id: BatchId,
    ) -> Result<Vec<WorkItemSummary>, ApplicationError> {
        Ok(self.store.list_work_items(batch_id)?)
    }

    pub fn get_work_item(
        &self,
        work_item_id: WorkItemId,
    ) -> Result<WorkItemSummary, ApplicationError> {
        self.store
            .find_work_item(work_item_id)?
            .ok_or_else(|| StorageError::Validation("作业项不存在".into()).into())
    }

    pub fn list_work_item_files(
        &self,
        work_item_id: WorkItemId,
    ) -> Result<Vec<WorkItemFileView>, ApplicationError> {
        let item = self.get_work_item(work_item_id)?;
        let algorithm_id = self
            .store
            .find_batch(item.batch_id)?
            .and_then(|batch| batch.algorithm_id);
        self.store
            .list_work_item_files(work_item_id)?
            .into_iter()
            .map(|file| {
                let matches = match &algorithm_id {
                    Some(algorithm_id) => {
                        self.store
                            .list_file_matches(item.batch_id, file.id, algorithm_id)?
                    }
                    None => Vec::new(),
                };
                Ok(WorkItemFileView {
                    match_count: matches.len() as u64,
                    highest_similarity: matches.first().map(|item| item.similarity),
                    file,
                })
            })
            .collect()
    }

    pub async fn import_batch<F>(
        &self,
        root: PathBuf,
        mut progress: F,
    ) -> Result<BatchImportSummary, ApplicationError>
    where
        F: FnMut(&BatchFileProgress) + Send,
    {
        let discovery = tokio::task::spawn_blocking(move || discover_batch(&root))
            .await
            .map_err(|error| ApplicationError::BackgroundTask(error.to_string()))??;
        let batch = self.store.create_batch(&discovery.batch_name)?;
        let mut summary = BatchImportSummary {
            batch,
            imported: 0,
            ready: 0,
            failed: 0,
            skipped: discovery.skipped.len() as u64,
            skipped_items: discovery.skipped.into_iter().map(Into::into).collect(),
        };

        for item_candidate in discovery.work_items {
            let item = self
                .store
                .create_work_item(summary.batch.id, &item_candidate.name)?;
            for candidate in item_candidate.files {
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
                let parser = self.parser_for(candidate.category);
                let parsed_key = self.objects.parsed_key(parser.id(), &object.content_hash);
                let outcome = self.store.insert_batch_file(NewBatchFile {
                    work_item_id: item.id,
                    name: candidate.name,
                    relative_path: candidate.relative_path,
                    category: candidate.category,
                    object,
                    parser_id: parser.id().into(),
                    reusable_parsed_key: self
                        .objects
                        .parsed_exists(&parsed_key)
                        .then_some(parsed_key),
                })?;
                let file = match outcome {
                    InsertBatchFileOutcome::Duplicate(file) => {
                        summary.skipped += 1;
                        summary.skipped_items.push(SkippedReferenceImport {
                            path: candidate.source_path.to_string_lossy().into_owned(),
                            reason: "同一作业项中已存在相同内容".into(),
                        });
                        progress(&BatchFileProgress {
                            batch_id: summary.batch.id,
                            work_item_id: item.id,
                            file,
                        });
                        continue;
                    }
                    InsertBatchFileOutcome::Inserted(file) => file,
                };
                summary.imported += 1;
                progress(&BatchFileProgress {
                    batch_id: summary.batch.id,
                    work_item_id: item.id,
                    file: file.clone(),
                });
                let file = if file.parse_status == ParseStatus::Ready {
                    file
                } else {
                    self.parse_file(summary.batch.id, item.id, file, &mut progress)
                        .await?
                };
                match file.parse_status {
                    ParseStatus::Ready => summary.ready += 1,
                    ParseStatus::Failed => summary.failed += 1,
                    ParseStatus::Pending | ParseStatus::Parsing => {}
                }
            }
        }
        summary.batch = self
            .store
            .find_batch(summary.batch.id)?
            .ok_or_else(|| StorageError::InvalidState("导入后的批次不存在".into()))?;
        Ok(summary)
    }

    pub async fn retry_file(
        &self,
        work_item_id: WorkItemId,
        file_id: FileId,
    ) -> Result<ManagedFile, ApplicationError> {
        let item = self.get_work_item(work_item_id)?;
        let file = self
            .store
            .find_work_item_file(work_item_id, file_id)?
            .ok_or_else(|| StorageError::Validation("作业文件不存在".into()))?;
        let file = self
            .parse_file(item.batch_id, item.id, file, &mut |_| {})
            .await?;
        self.store.clear_batch_results(item.batch_id)?;
        self.store
            .update_batch_detection(item.batch_id, DetectionStatus::Pending, None, None)?;
        Ok(file)
    }

    pub async fn run_detection<F>(
        &self,
        batch_id: BatchId,
        algorithm: Arc<dyn DetectionAlgorithm>,
        mut progress: F,
    ) -> Result<DetectionRunSummary, ApplicationError>
    where
        F: FnMut(&DetectionProgress) + Send,
    {
        let algorithm_id = algorithm.descriptor().id;
        self.store.update_batch_detection(
            batch_id,
            DetectionStatus::Running,
            Some(algorithm_id),
            None,
        )?;
        self.store.clear_batch_results(batch_id)?;
        let result = self
            .run_detection_inner(batch_id, algorithm, &mut progress)
            .await;
        match result {
            Ok(summary) => {
                self.store.update_batch_detection(
                    batch_id,
                    DetectionStatus::Ready,
                    Some(algorithm_id),
                    None,
                )?;
                Ok(summary)
            }
            Err(error) => {
                let message = error.to_string();
                let _ = self.store.update_batch_detection(
                    batch_id,
                    DetectionStatus::Failed,
                    Some(algorithm_id),
                    Some(&message),
                );
                Err(error)
            }
        }
    }

    pub fn list_file_matches(
        &self,
        batch_id: BatchId,
        file_id: FileId,
    ) -> Result<Vec<FileMatchSummary>, ApplicationError> {
        let batch = self
            .store
            .find_batch(batch_id)?
            .ok_or_else(|| StorageError::Validation("批次不存在".into()))?;
        let Some(algorithm_id) = batch.algorithm_id else {
            return Ok(Vec::new());
        };
        Ok(self
            .store
            .list_file_matches(batch_id, file_id, &algorithm_id)?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    pub async fn get_match_detail(
        &self,
        query_file_id: FileId,
        result_id: u64,
    ) -> Result<MatchDetail, ApplicationError> {
        let comparison = self
            .store
            .find_comparison(result_id, query_file_id)?
            .ok_or_else(|| StorageError::Validation("匹配结果不存在".into()))?;
        let query_file = self
            .store
            .find_managed_file(comparison.query_file_id)?
            .ok_or_else(|| StorageError::InvalidState("查询文件已不存在".into()))?;
        let source_file = self
            .store
            .find_managed_file(comparison.source_file_id)?
            .ok_or_else(|| StorageError::InvalidState("来源文件已不存在".into()))?;
        let query = self.read_parsed(&query_file).await?;
        let source = self.read_parsed(&source_file).await?;
        let risk_regions = comparison
            .risk_regions
            .into_iter()
            .map(|region| LocatedRiskRegion {
                query_location: locate_range(&query, region.query_range),
                source_location: region
                    .source_range
                    .and_then(|range| locate_range(&source, range)),
                query_range: region.query_range,
                source_range: region.source_range,
                score: region.score,
            })
            .collect();
        Ok(MatchDetail {
            result_id: comparison.id,
            query_file,
            source_file,
            query_text: query.text,
            source_text: source.text,
            similarity: comparison.similarity,
            algorithm_id: comparison.algorithm_id,
            risk_regions,
        })
    }

    async fn run_detection_inner<F>(
        &self,
        batch_id: BatchId,
        algorithm: Arc<dyn DetectionAlgorithm>,
        progress: &mut F,
    ) -> Result<DetectionRunSummary, ApplicationError>
    where
        F: FnMut(&DetectionProgress) + Send,
    {
        let batch_files: Vec<_> = self
            .store
            .list_batch_file_records(batch_id)?
            .into_iter()
            .filter(|record| record.file.parse_status == ParseStatus::Ready)
            .collect();
        if batch_files.is_empty() {
            return Err(StorageError::Validation("批次中没有解析成功的文件".into()).into());
        }
        let references = self.store.list_ready_reference_file_records()?;
        let mut batch_parsed = HashMap::new();
        for record in &batch_files {
            batch_parsed.insert(record.file.id, self.read_parsed(&record.file).await?);
        }
        let mut reference_parsed = HashMap::new();
        for record in &references {
            reference_parsed.insert(record.file.id, self.read_parsed(&record.file).await?);
        }

        let supported = algorithm.descriptor().supported_categories;
        let total_files = batch_files
            .iter()
            .filter(|record| supported.contains(&record.file.category))
            .count() as u64;
        let mut summary = DetectionRunSummary {
            batch_id,
            algorithm_id: algorithm.descriptor().id.into(),
            query_files: total_files,
            compared_pairs: 0,
            matches: 0,
        };
        let mut processed_files = 0;

        for category in [FileCategory::Document, FileCategory::Code] {
            if !supported.contains(&category) {
                continue;
            }
            let category_batch: Vec<_> = batch_files
                .iter()
                .filter(|record| record.file.category == category)
                .collect();
            let category_references: Vec<_> = references
                .iter()
                .filter(|record| record.file.category == category)
                .collect();
            let pipeline = DetectionPipeline::new(algorithm.as_ref());
            let mut batch_prepared = HashMap::new();
            for record in &category_batch {
                batch_prepared.insert(
                    record.file.id,
                    pipeline.preprocess(&batch_parsed[&record.file.id])?,
                );
            }
            let mut reference_prepared = HashMap::new();
            for record in &category_references {
                reference_prepared.insert(
                    record.file.id,
                    pipeline.preprocess(&reference_parsed[&record.file.id])?,
                );
            }
            let batch_corpus: Vec<_> = batch_prepared.values().cloned().collect();
            let reference_corpus: Vec<_> = reference_prepared.values().cloned().collect();
            let batch_index = pipeline.build_index(&batch_corpus)?;
            let reference_index = pipeline.build_index(&reference_corpus)?;
            let batch_metadata: HashMap<_, _> = category_batch
                .iter()
                .map(|record| (record.file.id, *record))
                .collect();
            let reference_metadata: HashMap<_, _> = category_references
                .iter()
                .map(|record| (record.file.id, *record))
                .collect();

            for query_record in category_batch {
                let query = &batch_prepared[&query_record.file.id];
                for candidate in batch_index.retrieve(query, RETRIEVAL_LIMIT)? {
                    let Some(source_record) = batch_metadata.get(&candidate.file_id) else {
                        continue;
                    };
                    if source_record.work_item_id == query_record.work_item_id {
                        continue;
                    }
                    let source = &batch_prepared[&candidate.file_id];
                    summary.compared_pairs += 1;
                    let evidence = pipeline.compare(query, source)?;
                    if should_store(&evidence.risk_regions, evidence.similarity) {
                        self.store.insert_comparison(&NewComparison {
                            batch_id,
                            query_file_id: query_record.file.id,
                            source_file_id: candidate.file_id,
                            scope: ComparisonScope::WithinBatch { batch_id },
                            algorithm_id: summary.algorithm_id.clone(),
                            similarity: evidence.similarity.clamp(0.0, 1.0),
                            risk_regions: evidence.risk_regions,
                        })?;
                        summary.matches += 1;
                    }
                }
                for candidate in reference_index.retrieve(query, RETRIEVAL_LIMIT)? {
                    let Some(source_record) = reference_metadata.get(&candidate.file_id) else {
                        continue;
                    };
                    let source = &reference_prepared[&candidate.file_id];
                    summary.compared_pairs += 1;
                    let evidence = pipeline.compare(query, source)?;
                    if should_store(&evidence.risk_regions, evidence.similarity) {
                        self.store.insert_comparison(&NewComparison {
                            batch_id,
                            query_file_id: query_record.file.id,
                            source_file_id: candidate.file_id,
                            scope: ComparisonScope::ReferenceLibrary {
                                library_id: source_record.library_id,
                            },
                            algorithm_id: summary.algorithm_id.clone(),
                            similarity: evidence.similarity.clamp(0.0, 1.0),
                            risk_regions: evidence.risk_regions,
                        })?;
                        summary.matches += 1;
                    }
                }
                processed_files += 1;
                progress(&DetectionProgress {
                    batch_id,
                    processed_files,
                    total_files,
                    matches: summary.matches,
                });
            }
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
        batch_id: BatchId,
        work_item_id: WorkItemId,
        file: ManagedFile,
        progress: &mut F,
    ) -> Result<ManagedFile, ApplicationError>
    where
        F: FnMut(&BatchFileProgress) + Send,
    {
        let parser = self.parser_for(file.category);
        let parsing = self.store.update_parse_status(
            file.id,
            ParseStatus::Parsing,
            parser.id(),
            None,
            None,
        )?;
        progress(&BatchFileProgress {
            batch_id,
            work_item_id,
            file: parsing.clone(),
        });
        let source = self.objects.object_path(&file.object_key);
        let parsed_key = self.objects.parsed_key(parser.id(), &file.content_hash);
        let result = parser.parse(&parsing, &source).await;
        let final_file = match result {
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
        progress(&BatchFileProgress {
            batch_id,
            work_item_id,
            file: final_file.clone(),
        });
        Ok(final_file)
    }

    async fn read_parsed(&self, file: &ManagedFile) -> Result<ParsedFile, ApplicationError> {
        let key = file
            .parsed_key
            .as_deref()
            .ok_or_else(|| StorageError::InvalidState("文件缺少解析产物".into()))?;
        Ok(self.objects.read_parsed(key, file.id).await?)
    }
}

fn locate_range(file: &ParsedFile, range: TextRange) -> Option<RiskLocation> {
    let overlaps = |mapping: &&crate::parser::LocationMapping| {
        mapping.text_range.start < range.end && range.start < mapping.text_range.end
    };

    match file.category {
        FileCategory::Document => {
            let mut pages = file
                .locations
                .iter()
                .filter(overlaps)
                .filter_map(|mapping| {
                    if let SourceLocation::Document {
                        page: Some(page), ..
                    } = &mapping.source
                    {
                        Some(*page)
                    } else {
                        None
                    }
                });
            let start_page = pages.next()?;
            let end_page = pages.last().unwrap_or(start_page);
            Some(RiskLocation::Document {
                start_page,
                end_page,
            })
        }
        FileCategory::Code => {
            let mut lines = file
                .locations
                .iter()
                .filter(overlaps)
                .filter_map(|mapping| {
                    if let SourceLocation::Code { line, .. } = &mapping.source {
                        Some(*line)
                    } else {
                        None
                    }
                });
            let start_line = lines.next()?;
            let end_line = lines.last().unwrap_or(start_line);
            Some(RiskLocation::Code {
                start_line,
                end_line,
            })
        }
    }
}

fn should_store(risk_regions: &[RiskRegion], similarity: f32) -> bool {
    let matched_bytes: u64 = risk_regions
        .iter()
        .map(|region| {
            region
                .query_range
                .end
                .saturating_sub(region.query_range.start)
        })
        .sum();
    similarity.is_finite() && similarity >= MIN_SIMILARITY && matched_bytes >= MIN_MATCHED_BYTES
}

impl From<StoredMatchSummary> for FileMatchSummary {
    fn from(value: StoredMatchSummary) -> Self {
        let source_kind = match value.scope {
            ComparisonScope::WithinBatch { .. } => "batch",
            ComparisonScope::ReferenceLibrary { .. } => "reference",
        };
        Self {
            id: value.id,
            source_file_id: value.source_file_id,
            source_name: value.source_name,
            source_path: value.source_path,
            source_kind,
            source_container: value.scope_name,
            source_work_item_name: value.source_work_item_name,
            algorithm_id: value.algorithm_id,
            similarity: value.similarity,
            risk_count: value.risk_count,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use crate::application::AppCore;
    use crate::parser::LocationMapping;

    use super::*;

    #[test]
    fn locates_document_risk_across_pages() {
        let file = ParsedFile {
            file_id: FileId(1),
            category: FileCategory::Document,
            text: "first\n\nsecond".into(),
            locations: vec![
                LocationMapping {
                    text_range: TextRange { start: 0, end: 5 },
                    source: SourceLocation::Document {
                        page: Some(1),
                        block: None,
                    },
                },
                LocationMapping {
                    text_range: TextRange { start: 7, end: 13 },
                    source: SourceLocation::Document {
                        page: Some(2),
                        block: None,
                    },
                },
            ],
        };

        assert_eq!(
            locate_range(&file, TextRange { start: 3, end: 9 }),
            Some(RiskLocation::Document {
                start_page: 1,
                end_page: 2,
            })
        );
    }

    #[test]
    fn locates_code_risk_across_lines() {
        let file = ParsedFile {
            file_id: FileId(1),
            category: FileCategory::Code,
            text: "first\nsecond\nthird".into(),
            locations: vec![
                LocationMapping {
                    text_range: TextRange { start: 0, end: 6 },
                    source: SourceLocation::Code { line: 1, column: 1 },
                },
                LocationMapping {
                    text_range: TextRange { start: 6, end: 13 },
                    source: SourceLocation::Code { line: 2, column: 1 },
                },
                LocationMapping {
                    text_range: TextRange { start: 13, end: 18 },
                    source: SourceLocation::Code { line: 3, column: 1 },
                },
            ],
        };

        assert_eq!(
            locate_range(&file, TextRange { start: 7, end: 15 }),
            Some(RiskLocation::Code {
                start_line: 2,
                end_line: 3,
            })
        );
    }

    #[test]
    fn serializes_risk_locations_for_the_frontend() {
        let value = serde_json::to_value(RiskLocation::Document {
            start_page: 2,
            end_page: 4,
        })
        .unwrap();

        assert_eq!(value["kind"], "document");
        assert_eq!(value["startPage"], 2);
        assert_eq!(value["endPage"], 4);
    }

    #[tokio::test]
    async fn imports_batch_and_runs_all_algorithms_against_both_scopes() {
        let directory = tempdir().unwrap();
        let data_root = directory.path().join("data");
        let batch_root = directory.path().join("第一次作业");
        fs::create_dir_all(batch_root.join("作业甲")).unwrap();
        fs::create_dir_all(batch_root.join("作业乙")).unwrap();
        let shared = "fn calculate_total(values: Vec<i32>) -> i32 {\n    let mut total = 0;\n    for value in values { total += value; }\n    total\n}\n";
        fs::write(batch_root.join("作业甲/main.rs"), shared).unwrap();
        fs::write(
            batch_root.join("作业乙/answer.rs"),
            format!("// changed heading\n{shared}"),
        )
        .unwrap();
        let reference_path = directory.path().join("reference.rs");
        fs::write(&reference_path, shared).unwrap();

        let core = AppCore::open(&data_root).unwrap();
        let library = core
            .references()
            .create_library("往届代码", FileCategory::Code)
            .unwrap();
        core.references()
            .import_paths(library.id, vec![reference_path], |_| {})
            .await
            .unwrap();
        let imported = core
            .batches()
            .import_batch(batch_root, |_| {})
            .await
            .unwrap();
        assert_eq!(imported.batch.work_item_count, 2);
        assert_eq!(imported.ready, 2);

        let items = core.batches().list_work_items(imported.batch.id).unwrap();
        let query_file = core.batches().list_work_item_files(items[0].id).unwrap()[0]
            .file
            .clone();
        let algorithm_ids: Vec<_> = core
            .algorithms()
            .descriptors()
            .into_iter()
            .map(|descriptor| descriptor.id)
            .collect();
        assert_eq!(algorithm_ids.len(), 3);

        for algorithm_id in algorithm_ids {
            let algorithm = core.algorithms().resolve(algorithm_id).unwrap();
            let run = core
                .batches()
                .run_detection(imported.batch.id, algorithm, |_| {})
                .await
                .unwrap();
            assert!(run.compared_pairs >= 2, "{algorithm_id}");
            let matches = core
                .batches()
                .list_file_matches(imported.batch.id, query_file.id)
                .unwrap();
            assert!(matches.iter().any(|item| item.source_kind == "batch"));
            assert!(matches.iter().any(|item| item.source_kind == "reference"));
            let detail = core
                .batches()
                .get_match_detail(query_file.id, matches[0].id)
                .await
                .unwrap();
            assert!(!detail.risk_regions.is_empty());
            for region in detail.risk_regions {
                assert!(region.query_location.is_some());
                assert!(
                    detail
                        .query_text
                        .get(region.query_range.start as usize..region.query_range.end as usize)
                        .is_some()
                );
            }
        }

        drop(core);
        let reopened = AppCore::open(&data_root).unwrap();
        let persisted = reopened
            .batches()
            .list_file_matches(imported.batch.id, query_file.id)
            .unwrap();
        assert!(!persisted.is_empty());
        assert_eq!(
            reopened
                .batches()
                .list_batches()
                .unwrap()
                .into_iter()
                .find(|batch| batch.id == imported.batch.id)
                .unwrap()
                .detection_status,
            DetectionStatus::Ready
        );

        let added_reference = directory.path().join("new-reference.rs");
        fs::write(
            &added_reference,
            "fn another_reference() { println!(\"new corpus entry\"); }\n",
        )
        .unwrap();
        reopened
            .references()
            .import_paths(library.id, vec![added_reference], |_| {})
            .await
            .unwrap();
        assert_eq!(
            reopened
                .batches()
                .list_batches()
                .unwrap()
                .into_iter()
                .find(|batch| batch.id == imported.batch.id)
                .unwrap()
                .detection_status,
            DetectionStatus::Pending
        );
        assert!(
            reopened
                .batches()
                .list_file_matches(imported.batch.id, query_file.id)
                .unwrap()
                .is_empty()
        );
    }
}
