use rusqlite::{OptionalExtension, params};

use crate::domain::{
    AnalysisUnitRange, BatchId, BatchSummary, ComparisonScope, DetectionStatus, FileCategory,
    FileId, ManagedFile, ParseStatus, ReferenceLibraryId, RiskRegion, WorkItemId, WorkItemSummary,
};

use super::sqlite::{conversion_error, map_file, now, parse_category_sql, prune_orphan_file};
use super::{CleanupKeys, ImportedObject, SqliteStore, StorageError};

#[derive(Debug, Clone)]
pub struct NewBatchFile {
    pub work_item_id: WorkItemId,
    pub name: String,
    pub relative_path: String,
    pub category: FileCategory,
    pub object: ImportedObject,
    pub parser_id: String,
    pub reusable_parsed_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InsertBatchFileOutcome {
    Inserted(ManagedFile),
    Duplicate(ManagedFile),
}

#[derive(Debug, Clone)]
pub struct BatchFileRecord {
    pub work_item_id: WorkItemId,
    pub work_item_name: String,
    pub file: ManagedFile,
}

#[derive(Debug, Clone)]
pub struct ReferenceFileRecord {
    pub library_id: ReferenceLibraryId,
    pub library_name: String,
    pub file: ManagedFile,
}

#[derive(Debug, Clone)]
pub struct NewComparison {
    pub batch_id: BatchId,
    pub query_file_id: FileId,
    pub source_file_id: FileId,
    pub scope: ComparisonScope,
    pub algorithm_id: String,
    pub similarity: f32,
    pub query_unit_count: u64,
    pub matched_unit_count: u64,
    pub matched_unit_ranges: Vec<AnalysisUnitRange>,
    pub risk_regions: Vec<RiskRegion>,
}

#[derive(Debug, Clone)]
pub struct StoredComparison {
    pub id: u64,
    pub batch_id: BatchId,
    pub query_file_id: FileId,
    pub source_file_id: FileId,
    pub scope: ComparisonScope,
    pub algorithm_id: String,
    pub similarity: f32,
    pub query_unit_count: u64,
    pub matched_unit_count: u64,
    pub matched_unit_ranges: Vec<AnalysisUnitRange>,
    pub risk_regions: Vec<RiskRegion>,
}

#[derive(Debug, Clone)]
pub struct StoredFileAnalysis {
    pub file_id: FileId,
    pub total_units: u64,
}

#[derive(Debug, Clone)]
pub struct StoredMatchSummary {
    pub id: u64,
    pub query_file_id: FileId,
    pub source_file_id: FileId,
    pub source_name: String,
    pub source_path: String,
    pub scope: ComparisonScope,
    pub scope_name: String,
    pub source_work_item_id: Option<WorkItemId>,
    pub source_work_item_name: Option<String>,
    pub algorithm_id: String,
    pub similarity: f32,
    pub matched_unit_count: u64,
    pub matched_unit_ranges: Vec<AnalysisUnitRange>,
    pub risk_count: u64,
}

impl SqliteStore {
    pub fn list_batches(&self) -> Result<Vec<BatchSummary>, StorageError> {
        let connection = self.connection.lock()?;
        let mut statement = connection.prepare(
            "SELECT b.id, b.name, b.detection_status, b.algorithm_id, b.detection_error,
                    COUNT(DISTINCT w.id), COUNT(wf.file_id),
                    COALESCE(SUM(CASE WHEN m.parse_status = 'ready' THEN 1 ELSE 0 END), 0)
             FROM batches b
             LEFT JOIN work_items w ON w.batch_id = b.id
             LEFT JOIN work_item_files wf ON wf.work_item_id = w.id
             LEFT JOIN managed_files m ON m.id = wf.file_id
             GROUP BY b.id
             ORDER BY b.updated_at DESC, b.id DESC",
        )?;
        let rows = statement.query_map([], |row| {
            let status: String = row.get(2)?;
            Ok(BatchSummary {
                id: BatchId(row.get::<_, i64>(0)? as u64),
                name: row.get(1)?,
                detection_status: parse_detection_status(&status, 2)?,
                algorithm_id: row.get(3)?,
                detection_error: row.get(4)?,
                work_item_count: row.get::<_, i64>(5)? as u64,
                file_count: row.get::<_, i64>(6)? as u64,
                ready_file_count: row.get::<_, i64>(7)? as u64,
                highest_file_similarity: None,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn find_batch(&self, id: BatchId) -> Result<Option<BatchSummary>, StorageError> {
        Ok(self
            .list_batches()?
            .into_iter()
            .find(|batch| batch.id == id))
    }

    pub fn create_batch(&self, name: &str) -> Result<BatchSummary, StorageError> {
        let name = validate_name(name, "批次名称")?;
        let connection = self.connection.lock()?;
        let timestamp = now();
        connection.execute(
            "INSERT INTO batches (
                 name, detection_status, algorithm_id, detection_error, created_at, updated_at
             ) VALUES (?1, 'pending', NULL, NULL, ?2, ?2)",
            params![name, timestamp],
        )?;
        let id = BatchId(connection.last_insert_rowid() as u64);
        drop(connection);
        self.find_batch(id)?
            .ok_or_else(|| StorageError::InvalidState("刚创建的批次不存在".into()))
    }

    pub fn rename_batch(&self, id: BatchId, name: &str) -> Result<BatchSummary, StorageError> {
        let name = validate_name(name, "批次名称")?;
        let connection = self.connection.lock()?;
        let changed = connection.execute(
            "UPDATE batches SET name = ?1, updated_at = ?2 WHERE id = ?3",
            params![name, now(), id.0 as i64],
        )?;
        drop(connection);
        if changed == 0 {
            return Err(StorageError::Validation("批次不存在".into()));
        }
        self.find_batch(id)?
            .ok_or_else(|| StorageError::InvalidState("重命名后的批次不存在".into()))
    }

    pub fn delete_batch(&self, id: BatchId) -> Result<CleanupKeys, StorageError> {
        let mut connection = self.connection.lock()?;
        let transaction = connection.transaction()?;
        let file_ids = {
            let mut statement = transaction.prepare(
                "SELECT wf.file_id
                 FROM work_item_files wf
                 INNER JOIN work_items w ON w.id = wf.work_item_id
                 WHERE w.batch_id = ?1",
            )?;
            let rows = statement.query_map([id.0 as i64], |row| row.get::<_, i64>(0))?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        let changed = transaction.execute("DELETE FROM batches WHERE id = ?1", [id.0 as i64])?;
        if changed == 0 {
            return Err(StorageError::Validation("批次不存在".into()));
        }
        let mut cleanup = CleanupKeys::default();
        for file_id in file_ids {
            cleanup.extend(prune_orphan_file(&transaction, file_id)?);
        }
        transaction.commit()?;
        Ok(cleanup)
    }

    pub fn create_work_item(
        &self,
        batch_id: BatchId,
        name: &str,
    ) -> Result<WorkItemSummary, StorageError> {
        let name = validate_name(name, "作业项名称")?;
        let connection = self.connection.lock()?;
        let timestamp = now();
        connection.execute(
            "INSERT INTO work_items (batch_id, name, created_at) VALUES (?1, ?2, ?3)",
            params![batch_id.0 as i64, name, timestamp],
        )?;
        let id = WorkItemId(connection.last_insert_rowid() as u64);
        connection.execute(
            "UPDATE batches SET updated_at = ?1 WHERE id = ?2",
            params![timestamp, batch_id.0 as i64],
        )?;
        Ok(WorkItemSummary {
            id,
            batch_id,
            name: name.into(),
            file_count: 0,
            ready_file_count: 0,
        })
    }

    pub fn list_work_items(&self, batch_id: BatchId) -> Result<Vec<WorkItemSummary>, StorageError> {
        let connection = self.connection.lock()?;
        let mut statement = connection.prepare(
            "SELECT w.id, w.batch_id, w.name, COUNT(wf.file_id),
                    COALESCE(SUM(CASE WHEN m.parse_status = 'ready' THEN 1 ELSE 0 END), 0)
             FROM work_items w
             LEFT JOIN work_item_files wf ON wf.work_item_id = w.id
             LEFT JOIN managed_files m ON m.id = wf.file_id
             WHERE w.batch_id = ?1
             GROUP BY w.id
             ORDER BY w.name COLLATE NOCASE, w.id",
        )?;
        let rows = statement.query_map([batch_id.0 as i64], |row| {
            Ok(WorkItemSummary {
                id: WorkItemId(row.get::<_, i64>(0)? as u64),
                batch_id: BatchId(row.get::<_, i64>(1)? as u64),
                name: row.get(2)?,
                file_count: row.get::<_, i64>(3)? as u64,
                ready_file_count: row.get::<_, i64>(4)? as u64,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn find_work_item(
        &self,
        work_item_id: WorkItemId,
    ) -> Result<Option<WorkItemSummary>, StorageError> {
        let connection = self.connection.lock()?;
        connection
            .query_row(
                "SELECT w.id, w.batch_id, w.name, COUNT(wf.file_id),
                        COALESCE(SUM(CASE WHEN m.parse_status = 'ready' THEN 1 ELSE 0 END), 0)
                 FROM work_items w
                 LEFT JOIN work_item_files wf ON wf.work_item_id = w.id
                 LEFT JOIN managed_files m ON m.id = wf.file_id
                 WHERE w.id = ?1
                 GROUP BY w.id",
                [work_item_id.0 as i64],
                |row| {
                    Ok(WorkItemSummary {
                        id: WorkItemId(row.get::<_, i64>(0)? as u64),
                        batch_id: BatchId(row.get::<_, i64>(1)? as u64),
                        name: row.get(2)?,
                        file_count: row.get::<_, i64>(3)? as u64,
                        ready_file_count: row.get::<_, i64>(4)? as u64,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn insert_batch_file(
        &self,
        new_file: NewBatchFile,
    ) -> Result<InsertBatchFileOutcome, StorageError> {
        let mut connection = self.connection.lock()?;
        let transaction = connection.transaction()?;
        let batch_id = transaction
            .query_row(
                "SELECT batch_id FROM work_items WHERE id = ?1",
                [new_file.work_item_id.0 as i64],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .ok_or_else(|| StorageError::Validation("作业项不存在".into()))?;
        let duplicate = transaction
            .query_row(
                "SELECT m.id, m.name, m.relative_path, m.object_key, m.parsed_key,
                        m.content_hash, m.extension, m.size_bytes, m.category,
                        m.parse_status, m.parse_error, m.parser_id
                 FROM managed_files m
                 INNER JOIN work_item_files wf ON wf.file_id = m.id
                 WHERE wf.work_item_id = ?1 AND m.content_hash = ?2
                 LIMIT 1",
                params![new_file.work_item_id.0 as i64, new_file.object.content_hash],
                map_file,
            )
            .optional()?;
        if let Some(file) = duplicate {
            transaction.commit()?;
            return Ok(InsertBatchFileOutcome::Duplicate(file));
        }

        let timestamp = now();
        let status = if new_file.reusable_parsed_key.is_some() {
            ParseStatus::Ready
        } else {
            ParseStatus::Pending
        };
        transaction.execute(
            "INSERT INTO managed_files (
                 name, relative_path, object_key, parsed_key, content_hash, extension,
                 size_bytes, category, parse_status, parse_error, parser_id,
                 created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, NULL, ?10, ?11, ?11)",
            params![
                new_file.name,
                new_file.relative_path,
                new_file.object.object_key,
                new_file.reusable_parsed_key,
                new_file.object.content_hash,
                new_file.object.extension,
                new_file.object.size_bytes as i64,
                new_file.category.as_str(),
                status.as_str(),
                new_file.parser_id,
                timestamp,
            ],
        )?;
        let file_id = FileId(transaction.last_insert_rowid() as u64);
        transaction.execute(
            "INSERT INTO work_item_files (work_item_id, file_id, created_at)
             VALUES (?1, ?2, ?3)",
            params![new_file.work_item_id.0 as i64, file_id.0 as i64, timestamp],
        )?;
        transaction.execute(
            "UPDATE batches SET updated_at = ?1 WHERE id = ?2",
            params![timestamp, batch_id],
        )?;
        let file = transaction.query_row(
            "SELECT id, name, relative_path, object_key, parsed_key, content_hash,
                    extension, size_bytes, category, parse_status, parse_error, parser_id
             FROM managed_files WHERE id = ?1",
            [file_id.0 as i64],
            map_file,
        )?;
        transaction.commit()?;
        Ok(InsertBatchFileOutcome::Inserted(file))
    }

    pub fn list_work_item_files(
        &self,
        work_item_id: WorkItemId,
    ) -> Result<Vec<ManagedFile>, StorageError> {
        let connection = self.connection.lock()?;
        let mut statement = connection.prepare(
            "SELECT m.id, m.name, m.relative_path, m.object_key, m.parsed_key,
                    m.content_hash, m.extension, m.size_bytes, m.category,
                    m.parse_status, m.parse_error, m.parser_id
             FROM managed_files m
             INNER JOIN work_item_files wf ON wf.file_id = m.id
             WHERE wf.work_item_id = ?1
             ORDER BY m.relative_path COLLATE NOCASE, m.id",
        )?;
        let rows = statement.query_map([work_item_id.0 as i64], map_file)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn find_work_item_file(
        &self,
        work_item_id: WorkItemId,
        file_id: FileId,
    ) -> Result<Option<ManagedFile>, StorageError> {
        let connection = self.connection.lock()?;
        connection
            .query_row(
                "SELECT m.id, m.name, m.relative_path, m.object_key, m.parsed_key,
                        m.content_hash, m.extension, m.size_bytes, m.category,
                        m.parse_status, m.parse_error, m.parser_id
                 FROM managed_files m
                 INNER JOIN work_item_files wf ON wf.file_id = m.id
                 WHERE wf.work_item_id = ?1 AND m.id = ?2",
                params![work_item_id.0 as i64, file_id.0 as i64],
                map_file,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn list_batch_file_records(
        &self,
        batch_id: BatchId,
    ) -> Result<Vec<BatchFileRecord>, StorageError> {
        let connection = self.connection.lock()?;
        let mut statement = connection.prepare(
            "SELECT w.id, w.name,
                    m.id, m.name, m.relative_path, m.object_key, m.parsed_key,
                    m.content_hash, m.extension, m.size_bytes, m.category,
                    m.parse_status, m.parse_error, m.parser_id
             FROM work_items w
             INNER JOIN work_item_files wf ON wf.work_item_id = w.id
             INNER JOIN managed_files m ON m.id = wf.file_id
             WHERE w.batch_id = ?1
             ORDER BY w.id, m.id",
        )?;
        let rows = statement.query_map([batch_id.0 as i64], |row| {
            Ok(BatchFileRecord {
                work_item_id: WorkItemId(row.get::<_, i64>(0)? as u64),
                work_item_name: row.get(1)?,
                file: map_file_offset(row, 2)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn list_ready_reference_file_records(
        &self,
    ) -> Result<Vec<ReferenceFileRecord>, StorageError> {
        let connection = self.connection.lock()?;
        let mut statement = connection.prepare(
            "SELECT l.id, l.name,
                    m.id, m.name, m.relative_path, m.object_key, m.parsed_key,
                    m.content_hash, m.extension, m.size_bytes, m.category,
                    m.parse_status, m.parse_error, m.parser_id
             FROM reference_libraries l
             INNER JOIN reference_entries e ON e.library_id = l.id
             INNER JOIN managed_files m ON m.id = e.file_id
             WHERE m.parse_status = 'ready'
             ORDER BY l.id, m.id",
        )?;
        let rows = statement.query_map([], |row| {
            Ok(ReferenceFileRecord {
                library_id: ReferenceLibraryId(row.get::<_, i64>(0)? as u64),
                library_name: row.get(1)?,
                file: map_file_offset(row, 2)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn find_managed_file(&self, file_id: FileId) -> Result<Option<ManagedFile>, StorageError> {
        let connection = self.connection.lock()?;
        connection
            .query_row(
                "SELECT id, name, relative_path, object_key, parsed_key, content_hash,
                        extension, size_bytes, category, parse_status, parse_error, parser_id
                 FROM managed_files WHERE id = ?1",
                [file_id.0 as i64],
                map_file,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn update_batch_detection(
        &self,
        batch_id: BatchId,
        status: DetectionStatus,
        algorithm_id: Option<&str>,
        error: Option<&str>,
    ) -> Result<(), StorageError> {
        let connection = self.connection.lock()?;
        let changed = connection.execute(
            "UPDATE batches
             SET detection_status = ?1, algorithm_id = ?2, detection_error = ?3, updated_at = ?4
             WHERE id = ?5",
            params![
                status.as_str(),
                algorithm_id,
                error,
                now(),
                batch_id.0 as i64
            ],
        )?;
        if changed == 0 {
            return Err(StorageError::Validation("批次不存在".into()));
        }
        Ok(())
    }

    pub fn clear_batch_results(&self, batch_id: BatchId) -> Result<(), StorageError> {
        let mut connection = self.connection.lock()?;
        let transaction = connection.transaction()?;
        transaction.execute(
            "DELETE FROM comparison_results WHERE batch_id = ?1",
            [batch_id.0 as i64],
        )?;
        transaction.execute(
            "DELETE FROM detection_file_stats WHERE batch_id = ?1",
            [batch_id.0 as i64],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn invalidate_all_detections(&self) -> Result<(), StorageError> {
        let mut connection = self.connection.lock()?;
        let transaction = connection.transaction()?;
        transaction.execute("DELETE FROM comparison_results", [])?;
        transaction.execute("DELETE FROM detection_file_stats", [])?;
        transaction.execute(
            "UPDATE batches
             SET detection_status = 'pending', algorithm_id = NULL,
                 detection_error = NULL, updated_at = ?1",
            [now()],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn upsert_file_analysis(
        &self,
        batch_id: BatchId,
        file_id: FileId,
        algorithm_id: &str,
        total_units: u64,
    ) -> Result<(), StorageError> {
        let connection = self.connection.lock()?;
        connection.execute(
            "INSERT INTO detection_file_stats (
                 batch_id, file_id, algorithm_id, total_units
             ) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(batch_id, file_id, algorithm_id)
             DO UPDATE SET total_units = excluded.total_units",
            params![
                batch_id.0 as i64,
                file_id.0 as i64,
                algorithm_id,
                total_units as i64,
            ],
        )?;
        Ok(())
    }

    pub fn list_work_item_file_analyses(
        &self,
        batch_id: BatchId,
        work_item_id: WorkItemId,
        algorithm_id: &str,
    ) -> Result<Vec<StoredFileAnalysis>, StorageError> {
        let connection = self.connection.lock()?;
        let mut statement = connection.prepare(
            "SELECT s.file_id, s.total_units
             FROM detection_file_stats s
             INNER JOIN work_item_files wf ON wf.file_id = s.file_id
             WHERE s.batch_id = ?1 AND wf.work_item_id = ?2 AND s.algorithm_id = ?3
             ORDER BY s.file_id",
        )?;
        let rows = statement.query_map(
            params![batch_id.0 as i64, work_item_id.0 as i64, algorithm_id],
            |row| {
                Ok(StoredFileAnalysis {
                    file_id: FileId(row.get::<_, i64>(0)? as u64),
                    total_units: row.get::<_, i64>(1)? as u64,
                })
            },
        )?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn insert_comparison(&self, comparison: &NewComparison) -> Result<(), StorageError> {
        let (scope_type, scope_id) = scope_parts(comparison.scope);
        let matched_unit_ranges = serde_json::to_string(&comparison.matched_unit_ranges)?;
        let risk_regions = serde_json::to_string(&comparison.risk_regions)?;
        let connection = self.connection.lock()?;
        connection.execute(
            "INSERT INTO comparison_results (
                 batch_id, query_file_id, source_file_id, scope_type, scope_id,
                 algorithm_id, similarity, query_unit_count, matched_unit_count,
                 matched_unit_ranges_json, risk_regions_json, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
             ON CONFLICT(batch_id, query_file_id, source_file_id, scope_type, scope_id, algorithm_id)
             DO UPDATE SET similarity = excluded.similarity,
                           query_unit_count = excluded.query_unit_count,
                           matched_unit_count = excluded.matched_unit_count,
                           matched_unit_ranges_json = excluded.matched_unit_ranges_json,
                           risk_regions_json = excluded.risk_regions_json,
                           created_at = excluded.created_at",
            params![
                comparison.batch_id.0 as i64,
                comparison.query_file_id.0 as i64,
                comparison.source_file_id.0 as i64,
                scope_type,
                scope_id,
                comparison.algorithm_id,
                comparison.similarity,
                comparison.query_unit_count as i64,
                comparison.matched_unit_count as i64,
                matched_unit_ranges,
                risk_regions,
                now(),
            ],
        )?;
        Ok(())
    }

    pub fn list_file_matches(
        &self,
        batch_id: BatchId,
        query_file_id: FileId,
        algorithm_id: &str,
    ) -> Result<Vec<StoredMatchSummary>, StorageError> {
        let connection = self.connection.lock()?;
        let mut statement = connection.prepare(
            "SELECT r.id, r.query_file_id, r.source_file_id, m.name, m.relative_path,
                    r.scope_type, r.scope_id, r.algorithm_id, r.similarity,
                    r.matched_unit_count, r.matched_unit_ranges_json, r.risk_regions_json,
                    CASE WHEN r.scope_type = 'reference' THEN rl.name ELSE b.name END,
                    CASE WHEN r.scope_type = 'batch' THEN wi.id ELSE NULL END,
                    CASE WHEN r.scope_type = 'batch' THEN wi.name ELSE NULL END
             FROM comparison_results r
             INNER JOIN managed_files m ON m.id = r.source_file_id
             LEFT JOIN reference_libraries rl
                    ON r.scope_type = 'reference' AND rl.id = r.scope_id
             LEFT JOIN batches b
                    ON r.scope_type = 'batch' AND b.id = r.scope_id
             LEFT JOIN work_item_files wf
                    ON r.scope_type = 'batch' AND wf.file_id = r.source_file_id
             LEFT JOIN work_items wi ON wi.id = wf.work_item_id
             WHERE r.batch_id = ?1 AND r.query_file_id = ?2 AND r.algorithm_id = ?3
            ORDER BY r.similarity DESC, r.id",
        )?;
        let rows = statement.query_map(
            params![batch_id.0 as i64, query_file_id.0 as i64, algorithm_id],
            map_match_summary,
        )?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn list_work_item_matches(
        &self,
        batch_id: BatchId,
        work_item_id: WorkItemId,
        algorithm_id: &str,
    ) -> Result<Vec<StoredMatchSummary>, StorageError> {
        let connection = self.connection.lock()?;
        let mut statement = connection.prepare(
            "SELECT r.id, r.query_file_id, r.source_file_id, m.name, m.relative_path,
                    r.scope_type, r.scope_id, r.algorithm_id, r.similarity,
                    r.matched_unit_count, r.matched_unit_ranges_json, r.risk_regions_json,
                    CASE WHEN r.scope_type = 'reference' THEN rl.name ELSE b.name END,
                    CASE WHEN r.scope_type = 'batch' THEN wi.id ELSE NULL END,
                    CASE WHEN r.scope_type = 'batch' THEN wi.name ELSE NULL END
             FROM comparison_results r
             INNER JOIN work_item_files query_wf ON query_wf.file_id = r.query_file_id
             INNER JOIN managed_files m ON m.id = r.source_file_id
             LEFT JOIN reference_libraries rl
                    ON r.scope_type = 'reference' AND rl.id = r.scope_id
             LEFT JOIN batches b
                    ON r.scope_type = 'batch' AND b.id = r.scope_id
             LEFT JOIN work_item_files source_wf
                    ON r.scope_type = 'batch' AND source_wf.file_id = r.source_file_id
             LEFT JOIN work_items wi ON wi.id = source_wf.work_item_id
             WHERE r.batch_id = ?1 AND query_wf.work_item_id = ?2 AND r.algorithm_id = ?3
             ORDER BY r.query_file_id, r.similarity DESC, r.id",
        )?;
        let rows = statement.query_map(
            params![batch_id.0 as i64, work_item_id.0 as i64, algorithm_id],
            map_match_summary,
        )?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn find_comparison(
        &self,
        result_id: u64,
        query_file_id: FileId,
    ) -> Result<Option<StoredComparison>, StorageError> {
        let connection = self.connection.lock()?;
        connection
            .query_row(
                "SELECT id, batch_id, query_file_id, source_file_id, scope_type,
                        scope_id, algorithm_id, similarity, query_unit_count,
                        matched_unit_count, matched_unit_ranges_json, risk_regions_json
                 FROM comparison_results
                 WHERE id = ?1 AND query_file_id = ?2",
                params![result_id as i64, query_file_id.0 as i64],
                |row| {
                    let scope_type: String = row.get(4)?;
                    let scope_id: i64 = row.get(5)?;
                    let matched_json: String = row.get(10)?;
                    let matched_unit_ranges =
                        serde_json::from_str(&matched_json).map_err(|error| {
                            rusqlite::Error::FromSqlConversionFailure(
                                10,
                                rusqlite::types::Type::Text,
                                Box::new(error),
                            )
                        })?;
                    let risk_json: String = row.get(11)?;
                    let risk_regions = serde_json::from_str(&risk_json).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            11,
                            rusqlite::types::Type::Text,
                            Box::new(error),
                        )
                    })?;
                    Ok(StoredComparison {
                        id: row.get::<_, i64>(0)? as u64,
                        batch_id: BatchId(row.get::<_, i64>(1)? as u64),
                        query_file_id: FileId(row.get::<_, i64>(2)? as u64),
                        source_file_id: FileId(row.get::<_, i64>(3)? as u64),
                        scope: parse_scope(&scope_type, scope_id, 4)?,
                        algorithm_id: row.get(6)?,
                        similarity: row.get(7)?,
                        query_unit_count: row.get::<_, i64>(8)? as u64,
                        matched_unit_count: row.get::<_, i64>(9)? as u64,
                        matched_unit_ranges,
                        risk_regions,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }
}

fn map_match_summary(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredMatchSummary> {
    let scope_type: String = row.get(5)?;
    let scope_id: i64 = row.get(6)?;
    let matched_json: String = row.get(10)?;
    let matched_unit_ranges = serde_json::from_str(&matched_json).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(10, rusqlite::types::Type::Text, Box::new(error))
    })?;
    let risk_json: String = row.get(11)?;
    let risks: Vec<RiskRegion> = serde_json::from_str(&risk_json).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(11, rusqlite::types::Type::Text, Box::new(error))
    })?;
    Ok(StoredMatchSummary {
        id: row.get::<_, i64>(0)? as u64,
        query_file_id: FileId(row.get::<_, i64>(1)? as u64),
        source_file_id: FileId(row.get::<_, i64>(2)? as u64),
        source_name: row.get(3)?,
        source_path: row.get(4)?,
        scope: parse_scope(&scope_type, scope_id, 5)?,
        algorithm_id: row.get(7)?,
        similarity: row.get(8)?,
        matched_unit_count: row.get::<_, i64>(9)? as u64,
        matched_unit_ranges,
        risk_count: risks.len() as u64,
        scope_name: row
            .get::<_, Option<String>>(12)?
            .unwrap_or_else(|| "已删除来源".into()),
        source_work_item_id: row
            .get::<_, Option<i64>>(13)?
            .map(|id| WorkItemId(id as u64)),
        source_work_item_name: row.get(14)?,
    })
}

fn map_file_offset(row: &rusqlite::Row<'_>, offset: usize) -> rusqlite::Result<ManagedFile> {
    let category: String = row.get(offset + 8)?;
    let status: String = row.get(offset + 9)?;
    Ok(ManagedFile {
        id: FileId(row.get::<_, i64>(offset)? as u64),
        name: row.get(offset + 1)?,
        relative_path: row.get(offset + 2)?,
        object_key: row.get(offset + 3)?,
        parsed_key: row.get(offset + 4)?,
        content_hash: row.get(offset + 5)?,
        extension: row.get(offset + 6)?,
        size_bytes: row.get::<_, i64>(offset + 7)? as u64,
        category: parse_category_sql(&category, offset + 8)?,
        parse_status: ParseStatus::parse(&status)
            .ok_or_else(|| conversion_error(offset + 9, &status))?,
        parse_error: row.get(offset + 10)?,
        parser_id: row.get(offset + 11)?,
    })
}

fn parse_detection_status(value: &str, column: usize) -> rusqlite::Result<DetectionStatus> {
    DetectionStatus::parse(value).ok_or_else(|| conversion_error(column, value))
}

fn scope_parts(scope: ComparisonScope) -> (&'static str, i64) {
    match scope {
        ComparisonScope::WithinBatch { batch_id } => ("batch", batch_id.0 as i64),
        ComparisonScope::ReferenceLibrary { library_id } => ("reference", library_id.0 as i64),
    }
}

fn parse_scope(value: &str, id: i64, column: usize) -> rusqlite::Result<ComparisonScope> {
    match value {
        "batch" => Ok(ComparisonScope::WithinBatch {
            batch_id: BatchId(id as u64),
        }),
        "reference" => Ok(ComparisonScope::ReferenceLibrary {
            library_id: ReferenceLibraryId(id as u64),
        }),
        _ => Err(conversion_error(column, value)),
    }
}

fn validate_name<'a>(name: &'a str, label: &str) -> Result<&'a str, StorageError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(StorageError::Validation(format!("{label}不能为空")));
    }
    if name.chars().count() > 120 {
        return Err(StorageError::Validation(format!(
            "{label}不能超过 120 个字符"
        )));
    }
    Ok(name)
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;
    use crate::storage::StorageLayout;

    fn imported(hash: &str) -> ImportedObject {
        ImportedObject {
            content_hash: hash.into(),
            object_key: format!("objects/00/{hash}.rs"),
            extension: "rs".into(),
            size_bytes: 12,
        }
    }

    #[test]
    fn persists_batch_work_items_and_files() {
        let directory = tempdir().unwrap();
        let store = SqliteStore::open(&StorageLayout::under(directory.path())).unwrap();
        let batch = store.create_batch("第一次作业").unwrap();
        let item = store.create_work_item(batch.id, "张三").unwrap();
        let file = match store
            .insert_batch_file(NewBatchFile {
                work_item_id: item.id,
                name: "main.rs".into(),
                relative_path: "张三/main.rs".into(),
                category: FileCategory::Code,
                object: imported("code"),
                parser_id: "code-text-v1".into(),
                reusable_parsed_key: None,
            })
            .unwrap()
        {
            InsertBatchFileOutcome::Inserted(file) => file,
            InsertBatchFileOutcome::Duplicate(_) => unreachable!(),
        };

        assert_eq!(store.list_work_items(batch.id).unwrap()[0].file_count, 1);
        assert_eq!(store.list_work_item_files(item.id).unwrap()[0].id, file.id);
        assert_eq!(store.list_batch_file_records(batch.id).unwrap().len(), 1);
    }

    #[test]
    fn batch_delete_returns_only_unreferenced_objects() {
        let directory = tempdir().unwrap();
        let store = SqliteStore::open(&StorageLayout::under(directory.path())).unwrap();
        let batch = store.create_batch("第二次作业").unwrap();
        let item = store.create_work_item(batch.id, "李四").unwrap();
        store
            .insert_batch_file(NewBatchFile {
                work_item_id: item.id,
                name: "main.rs".into(),
                relative_path: "李四/main.rs".into(),
                category: FileCategory::Code,
                object: imported("cleanup-batch"),
                parser_id: "code-text-v1".into(),
                reusable_parsed_key: None,
            })
            .unwrap();

        let cleanup = store.delete_batch(batch.id).unwrap();
        assert_eq!(cleanup.object_keys, vec!["objects/00/cleanup-batch.rs"]);
        assert!(store.list_batches().unwrap().is_empty());
    }
}
