use std::{
    sync::Mutex,
    time::Duration,
    time::{SystemTime, UNIX_EPOCH},
};

use rusqlite::{Connection, ErrorCode, OptionalExtension, Row, params};

use crate::domain::{
    FileCategory, FileId, ManagedFile, ParseStatus, ReferenceLibrary, ReferenceLibraryId,
    ReferenceLibrarySummary,
};

use super::{ImportedObject, StorageError, StorageLayout};

#[derive(Debug, Clone)]
pub struct NewReferenceFile {
    pub library_id: ReferenceLibraryId,
    pub name: String,
    pub relative_path: String,
    pub category: FileCategory,
    pub object: ImportedObject,
    pub parser_id: String,
    pub reusable_parsed_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InsertReferenceFileOutcome {
    Inserted(ManagedFile),
    Duplicate(ManagedFile),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CleanupKeys {
    pub object_keys: Vec<String>,
    pub parsed_keys: Vec<String>,
}

impl CleanupKeys {
    pub fn all(self) -> impl Iterator<Item = String> {
        self.object_keys.into_iter().chain(self.parsed_keys)
    }

    pub(super) fn extend(&mut self, other: Self) {
        self.object_keys.extend(other.object_keys);
        self.parsed_keys.extend(other.parsed_keys);
    }
}

pub struct SqliteStore {
    pub(super) connection: Mutex<Connection>,
}

impl SqliteStore {
    pub fn open(layout: &StorageLayout) -> Result<Self, StorageError> {
        layout.ensure()?;
        let connection = Connection::open(&layout.database)?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.busy_timeout(Duration::from_secs(5))?;
        migrate(&connection)?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    pub fn recover_interrupted_parses(&self) -> Result<u64, StorageError> {
        let connection = self.connection.lock()?;
        let changed = connection.execute(
            "UPDATE managed_files
             SET parse_status = 'failed',
                 parse_error = '上次解析被应用退出中断，请重试',
                 updated_at = ?1
             WHERE parse_status = 'parsing'",
            [now()],
        )?;
        Ok(changed as u64)
    }

    pub fn recover_interrupted_detections(&self) -> Result<u64, StorageError> {
        let connection = self.connection.lock()?;
        let changed = connection.execute(
            "UPDATE batches
             SET detection_status = 'failed',
                 detection_error = '上次查重被应用退出中断，请重新运行',
                 updated_at = ?1
             WHERE detection_status = 'running'",
            [now()],
        )?;
        Ok(changed as u64)
    }

    pub fn list_reference_libraries(&self) -> Result<Vec<ReferenceLibrarySummary>, StorageError> {
        let connection = self.connection.lock()?;
        let mut statement = connection.prepare(
            "SELECT l.id, l.name, l.category, COUNT(e.file_id)
             FROM reference_libraries l
             LEFT JOIN reference_entries e ON e.library_id = l.id
             GROUP BY l.id
             ORDER BY l.updated_at DESC, l.id DESC",
        )?;
        let rows = statement.query_map([], |row| {
            Ok(ReferenceLibrarySummary {
                library: map_library(row)?,
                file_count: row.get::<_, i64>(3)? as u64,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn find_reference_library(
        &self,
        id: ReferenceLibraryId,
    ) -> Result<Option<ReferenceLibrary>, StorageError> {
        let connection = self.connection.lock()?;
        connection
            .query_row(
                "SELECT id, name, category FROM reference_libraries WHERE id = ?1",
                [id.0 as i64],
                map_library,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn create_reference_library(
        &self,
        name: &str,
        category: FileCategory,
    ) -> Result<ReferenceLibrary, StorageError> {
        let name = validate_library_name(name)?;
        let connection = self.connection.lock()?;
        let timestamp = now();
        if let Err(error) = connection.execute(
            "INSERT INTO reference_libraries (name, category, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?3)",
            params![name, category.as_str(), timestamp],
        ) {
            return Err(map_constraint(error, "同类型下已存在同名参考库"));
        }

        Ok(ReferenceLibrary {
            id: ReferenceLibraryId(connection.last_insert_rowid() as u64),
            name: name.to_owned(),
            category,
        })
    }

    pub fn rename_reference_library(
        &self,
        id: ReferenceLibraryId,
        name: &str,
    ) -> Result<ReferenceLibrary, StorageError> {
        let name = validate_library_name(name)?;
        let connection = self.connection.lock()?;
        if let Err(error) = connection.execute(
            "UPDATE reference_libraries SET name = ?1, updated_at = ?2 WHERE id = ?3",
            params![name, now(), id.0 as i64],
        ) {
            return Err(map_constraint(error, "同类型下已存在同名参考库"));
        }
        connection
            .query_row(
                "SELECT id, name, category FROM reference_libraries WHERE id = ?1",
                [id.0 as i64],
                map_library,
            )
            .optional()?
            .ok_or_else(|| StorageError::Validation("参考库不存在".into()))
    }

    pub fn delete_reference_library(
        &self,
        id: ReferenceLibraryId,
    ) -> Result<CleanupKeys, StorageError> {
        let mut connection = self.connection.lock()?;
        let transaction = connection.transaction()?;
        let file_ids = {
            let mut statement = transaction
                .prepare("SELECT file_id FROM reference_entries WHERE library_id = ?1")?;
            let rows = statement.query_map([id.0 as i64], |row| row.get::<_, i64>(0))?;
            rows.collect::<Result<Vec<_>, _>>()?
        };

        transaction.execute(
            "DELETE FROM comparison_results
             WHERE scope_type = 'reference' AND scope_id = ?1",
            [id.0 as i64],
        )?;

        let changed = transaction.execute(
            "DELETE FROM reference_libraries WHERE id = ?1",
            [id.0 as i64],
        )?;
        if changed == 0 {
            return Err(StorageError::Validation("参考库不存在".into()));
        }

        let mut cleanup = CleanupKeys::default();
        for file_id in file_ids {
            cleanup.extend(prune_orphan_file(&transaction, file_id)?);
        }
        transaction.commit()?;
        Ok(cleanup)
    }

    pub fn list_reference_files(
        &self,
        library_id: ReferenceLibraryId,
    ) -> Result<Vec<ManagedFile>, StorageError> {
        let connection = self.connection.lock()?;
        let mut statement = connection.prepare(
            "SELECT m.id, m.name, m.relative_path, m.object_key, m.parsed_key,
                    m.content_hash, m.extension, m.size_bytes, m.category,
                    m.parse_status, m.parse_error, m.parser_id
             FROM managed_files m
             INNER JOIN reference_entries e ON e.file_id = m.id
             WHERE e.library_id = ?1
             ORDER BY e.created_at DESC, m.id DESC",
        )?;
        let rows = statement.query_map([library_id.0 as i64], map_file)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn find_reference_file(
        &self,
        library_id: ReferenceLibraryId,
        file_id: FileId,
    ) -> Result<Option<ManagedFile>, StorageError> {
        let connection = self.connection.lock()?;
        connection
            .query_row(
                "SELECT m.id, m.name, m.relative_path, m.object_key, m.parsed_key,
                        m.content_hash, m.extension, m.size_bytes, m.category,
                        m.parse_status, m.parse_error, m.parser_id
                 FROM managed_files m
                 INNER JOIN reference_entries e ON e.file_id = m.id
                 WHERE e.library_id = ?1 AND m.id = ?2",
                params![library_id.0 as i64, file_id.0 as i64],
                map_file,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn insert_reference_file(
        &self,
        new_file: NewReferenceFile,
    ) -> Result<InsertReferenceFileOutcome, StorageError> {
        let mut connection = self.connection.lock()?;
        let transaction = connection.transaction()?;
        let library_category = transaction
            .query_row(
                "SELECT category FROM reference_libraries WHERE id = ?1",
                [new_file.library_id.0 as i64],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .ok_or_else(|| StorageError::Validation("参考库不存在".into()))?;
        if parse_category(&library_category)? != new_file.category {
            return Err(StorageError::Validation(
                "文件类型与参考库类型不匹配".into(),
            ));
        }

        let duplicate = transaction
            .query_row(
                "SELECT m.id, m.name, m.relative_path, m.object_key, m.parsed_key,
                        m.content_hash, m.extension, m.size_bytes, m.category,
                        m.parse_status, m.parse_error, m.parser_id
                 FROM managed_files m
                 INNER JOIN reference_entries e ON e.file_id = m.id
                 WHERE e.library_id = ?1 AND m.content_hash = ?2
                 LIMIT 1",
                params![new_file.library_id.0 as i64, new_file.object.content_hash],
                map_file,
            )
            .optional()?;
        if let Some(file) = duplicate {
            transaction.commit()?;
            return Ok(InsertReferenceFileOutcome::Duplicate(file));
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
            "INSERT INTO reference_entries (library_id, file_id, created_at)
             VALUES (?1, ?2, ?3)",
            params![new_file.library_id.0 as i64, file_id.0 as i64, timestamp],
        )?;
        transaction.execute(
            "UPDATE reference_libraries SET updated_at = ?1 WHERE id = ?2",
            params![timestamp, new_file.library_id.0 as i64],
        )?;
        let file = transaction.query_row(
            "SELECT id, name, relative_path, object_key, parsed_key, content_hash,
                    extension, size_bytes, category, parse_status, parse_error, parser_id
             FROM managed_files WHERE id = ?1",
            [file_id.0 as i64],
            map_file,
        )?;
        transaction.commit()?;
        Ok(InsertReferenceFileOutcome::Inserted(file))
    }

    pub fn update_parse_status(
        &self,
        file_id: FileId,
        status: ParseStatus,
        parser_id: &str,
        parsed_key: Option<&str>,
        error: Option<&str>,
    ) -> Result<ManagedFile, StorageError> {
        if status == ParseStatus::Ready && parsed_key.is_none() {
            return Err(StorageError::InvalidState(
                "解析成功状态必须包含解析产物".into(),
            ));
        }

        let connection = self.connection.lock()?;
        let changed = connection.execute(
            "UPDATE managed_files
             SET parse_status = ?1, parser_id = ?2, parsed_key = ?3,
                 parse_error = ?4, updated_at = ?5
             WHERE id = ?6",
            params![
                status.as_str(),
                parser_id,
                parsed_key,
                error,
                now(),
                file_id.0 as i64,
            ],
        )?;
        if changed == 0 {
            return Err(StorageError::Validation("参考文件不存在".into()));
        }
        connection
            .query_row(
                "SELECT id, name, relative_path, object_key, parsed_key, content_hash,
                        extension, size_bytes, category, parse_status, parse_error, parser_id
                 FROM managed_files WHERE id = ?1",
                [file_id.0 as i64],
                map_file,
            )
            .map_err(Into::into)
    }

    pub fn delete_reference_file(
        &self,
        library_id: ReferenceLibraryId,
        file_id: FileId,
    ) -> Result<CleanupKeys, StorageError> {
        let mut connection = self.connection.lock()?;
        let transaction = connection.transaction()?;
        let changed = transaction.execute(
            "DELETE FROM reference_entries WHERE library_id = ?1 AND file_id = ?2",
            params![library_id.0 as i64, file_id.0 as i64],
        )?;
        if changed == 0 {
            return Err(StorageError::Validation("参考文件不存在".into()));
        }
        transaction.execute(
            "DELETE FROM comparison_results WHERE source_file_id = ?1",
            [file_id.0 as i64],
        )?;
        let cleanup = prune_orphan_file(&transaction, file_id.0 as i64)?;
        transaction.execute(
            "UPDATE reference_libraries SET updated_at = ?1 WHERE id = ?2",
            params![now(), library_id.0 as i64],
        )?;
        transaction.commit()?;
        Ok(cleanup)
    }
}

fn migrate(connection: &Connection) -> Result<(), StorageError> {
    let prior_version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS reference_libraries (
             id INTEGER PRIMARY KEY AUTOINCREMENT,
             name TEXT NOT NULL COLLATE NOCASE,
             category TEXT NOT NULL CHECK(category IN ('document', 'code')),
             created_at INTEGER NOT NULL,
             updated_at INTEGER NOT NULL,
             UNIQUE(name, category)
         );

         CREATE TABLE IF NOT EXISTS managed_files (
             id INTEGER PRIMARY KEY AUTOINCREMENT,
             name TEXT NOT NULL,
             relative_path TEXT NOT NULL,
             object_key TEXT NOT NULL,
             parsed_key TEXT,
             content_hash TEXT NOT NULL,
             extension TEXT NOT NULL,
             size_bytes INTEGER NOT NULL,
             category TEXT NOT NULL CHECK(category IN ('document', 'code')),
             parse_status TEXT NOT NULL CHECK(parse_status IN ('pending', 'parsing', 'ready', 'failed')),
             parse_error TEXT,
             parser_id TEXT,
             created_at INTEGER NOT NULL,
             updated_at INTEGER NOT NULL
         );

         CREATE INDEX IF NOT EXISTS idx_managed_files_hash
             ON managed_files(content_hash);

         CREATE TABLE IF NOT EXISTS reference_entries (
             library_id INTEGER NOT NULL,
             file_id INTEGER NOT NULL,
             created_at INTEGER NOT NULL,
             PRIMARY KEY(library_id, file_id),
             FOREIGN KEY(library_id) REFERENCES reference_libraries(id) ON DELETE CASCADE,
             FOREIGN KEY(file_id) REFERENCES managed_files(id) ON DELETE CASCADE
         );

         CREATE TABLE IF NOT EXISTS batches (
             id INTEGER PRIMARY KEY AUTOINCREMENT,
             name TEXT NOT NULL,
             detection_status TEXT NOT NULL DEFAULT 'pending'
                 CHECK(detection_status IN ('pending', 'running', 'ready', 'failed')),
             algorithm_id TEXT,
             detection_error TEXT,
             created_at INTEGER NOT NULL,
             updated_at INTEGER NOT NULL
         );

         CREATE TABLE IF NOT EXISTS work_items (
             id INTEGER PRIMARY KEY AUTOINCREMENT,
             batch_id INTEGER NOT NULL,
             name TEXT NOT NULL,
             created_at INTEGER NOT NULL,
             UNIQUE(batch_id, name),
             FOREIGN KEY(batch_id) REFERENCES batches(id) ON DELETE CASCADE
         );

         CREATE TABLE IF NOT EXISTS work_item_files (
             work_item_id INTEGER NOT NULL,
             file_id INTEGER NOT NULL,
             created_at INTEGER NOT NULL,
             PRIMARY KEY(work_item_id, file_id),
             FOREIGN KEY(work_item_id) REFERENCES work_items(id) ON DELETE CASCADE,
             FOREIGN KEY(file_id) REFERENCES managed_files(id) ON DELETE CASCADE
         );

         CREATE INDEX IF NOT EXISTS idx_work_items_batch
             ON work_items(batch_id);
         CREATE INDEX IF NOT EXISTS idx_work_item_files_file
             ON work_item_files(file_id);

         CREATE TABLE IF NOT EXISTS comparison_results (
             id INTEGER PRIMARY KEY AUTOINCREMENT,
             batch_id INTEGER NOT NULL,
             query_file_id INTEGER NOT NULL,
             source_file_id INTEGER NOT NULL,
             scope_type TEXT NOT NULL CHECK(scope_type IN ('batch', 'reference')),
             scope_id INTEGER NOT NULL,
             algorithm_id TEXT NOT NULL,
             similarity REAL NOT NULL,
             query_unit_count INTEGER NOT NULL,
             matched_unit_count INTEGER NOT NULL,
             matched_unit_ranges_json TEXT NOT NULL,
             risk_regions_json TEXT NOT NULL,
             created_at INTEGER NOT NULL,
             UNIQUE(batch_id, query_file_id, source_file_id, scope_type, scope_id, algorithm_id),
             FOREIGN KEY(batch_id) REFERENCES batches(id) ON DELETE CASCADE,
             FOREIGN KEY(query_file_id) REFERENCES managed_files(id) ON DELETE CASCADE,
             FOREIGN KEY(source_file_id) REFERENCES managed_files(id) ON DELETE CASCADE
         );

         CREATE INDEX IF NOT EXISTS idx_comparison_results_query
             ON comparison_results(batch_id, query_file_id, algorithm_id, similarity DESC);

         CREATE TABLE IF NOT EXISTS detection_file_stats (
             batch_id INTEGER NOT NULL,
             file_id INTEGER NOT NULL,
             algorithm_id TEXT NOT NULL,
             total_units INTEGER NOT NULL,
             PRIMARY KEY(batch_id, file_id, algorithm_id),
             FOREIGN KEY(batch_id) REFERENCES batches(id) ON DELETE CASCADE,
             FOREIGN KEY(file_id) REFERENCES managed_files(id) ON DELETE CASCADE
         );

         CREATE INDEX IF NOT EXISTS idx_detection_file_stats_batch
             ON detection_file_stats(batch_id, algorithm_id);",
    )?;

    let mut result_schema_changed = false;
    for (column, definition) in [
        ("query_unit_count", "INTEGER NOT NULL DEFAULT 0"),
        ("matched_unit_count", "INTEGER NOT NULL DEFAULT 0"),
        ("matched_unit_ranges_json", "TEXT NOT NULL DEFAULT '[]'"),
    ] {
        if !table_has_column(connection, "comparison_results", column)? {
            connection.execute(
                &format!("ALTER TABLE comparison_results ADD COLUMN {column} {definition}"),
                [],
            )?;
            result_schema_changed = true;
        }
    }

    // Version 4 changes comparison semantics: matched ranges now require
    // background-corpus IDF evidence. Old results must not be mixed with the new
    // coverage metric even though the table shape is unchanged.
    if prior_version < 4 || result_schema_changed {
        connection.execute_batch(
            "DELETE FROM comparison_results;
             DELETE FROM detection_file_stats;
             UPDATE batches
             SET detection_status = 'pending', algorithm_id = NULL,
                 detection_error = NULL;",
        )?;
    }
    connection.pragma_update(None, "user_version", 4)?;
    Ok(())
}

fn table_has_column(
    connection: &Connection,
    table: &str,
    column: &str,
) -> Result<bool, StorageError> {
    let mut statement = connection.prepare(&format!("PRAGMA table_info({table})"))?;
    let names = statement.query_map([], |row| row.get::<_, String>(1))?;
    for name in names {
        if name? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

fn map_library(row: &Row<'_>) -> rusqlite::Result<ReferenceLibrary> {
    let category: String = row.get(2)?;
    Ok(ReferenceLibrary {
        id: ReferenceLibraryId(row.get::<_, i64>(0)? as u64),
        name: row.get(1)?,
        category: parse_category_sql(&category, 2)?,
    })
}

pub(super) fn map_file(row: &Row<'_>) -> rusqlite::Result<ManagedFile> {
    let category: String = row.get(8)?;
    let status: String = row.get(9)?;
    Ok(ManagedFile {
        id: FileId(row.get::<_, i64>(0)? as u64),
        name: row.get(1)?,
        relative_path: row.get(2)?,
        object_key: row.get(3)?,
        parsed_key: row.get(4)?,
        content_hash: row.get(5)?,
        extension: row.get(6)?,
        size_bytes: row.get::<_, i64>(7)? as u64,
        category: parse_category_sql(&category, 8)?,
        parse_status: parse_status_sql(&status, 9)?,
        parse_error: row.get(10)?,
        parser_id: row.get(11)?,
    })
}

fn parse_category(value: &str) -> Result<FileCategory, StorageError> {
    FileCategory::parse(value)
        .ok_or_else(|| StorageError::InvalidState(format!("未知文件类型：{value}")))
}

pub(super) fn parse_category_sql(value: &str, column: usize) -> rusqlite::Result<FileCategory> {
    FileCategory::parse(value).ok_or_else(|| conversion_error(column, value))
}

fn parse_status_sql(value: &str, column: usize) -> rusqlite::Result<ParseStatus> {
    ParseStatus::parse(value).ok_or_else(|| conversion_error(column, value))
}

pub(super) fn conversion_error(column: usize, value: &str) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        column,
        rusqlite::types::Type::Text,
        Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("未知数据库枚举值：{value}"),
        )),
    )
}

fn validate_library_name(name: &str) -> Result<&str, StorageError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(StorageError::Validation("参考库名称不能为空".into()));
    }
    if name.chars().count() > 80 {
        return Err(StorageError::Validation(
            "参考库名称不能超过 80 个字符".into(),
        ));
    }
    Ok(name)
}

fn map_constraint(error: rusqlite::Error, message: &str) -> StorageError {
    match &error {
        rusqlite::Error::SqliteFailure(inner, _)
            if inner.code == ErrorCode::ConstraintViolation =>
        {
            StorageError::Validation(message.into())
        }
        _ => error.into(),
    }
}

pub(super) fn prune_orphan_file(
    transaction: &rusqlite::Transaction<'_>,
    file_id: i64,
) -> Result<CleanupKeys, StorageError> {
    let references: i64 = transaction.query_row(
        "SELECT
             (SELECT COUNT(*) FROM reference_entries WHERE file_id = ?1) +
             (SELECT COUNT(*) FROM work_item_files WHERE file_id = ?1)",
        [file_id],
        |row| row.get(0),
    )?;
    if references != 0 {
        return Ok(CleanupKeys::default());
    }

    let keys = transaction
        .query_row(
            "SELECT object_key, parsed_key FROM managed_files WHERE id = ?1",
            [file_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
        )
        .optional()?;
    let Some((object_key, parsed_key)) = keys else {
        return Ok(CleanupKeys::default());
    };
    transaction.execute("DELETE FROM managed_files WHERE id = ?1", [file_id])?;

    let object_references: i64 = transaction.query_row(
        "SELECT COUNT(*) FROM managed_files WHERE object_key = ?1",
        [&object_key],
        |row| row.get(0),
    )?;
    let mut cleanup = CleanupKeys::default();
    if object_references == 0 {
        cleanup.object_keys.push(object_key);
    }

    if let Some(parsed_key) = parsed_key {
        let parsed_references: i64 = transaction.query_row(
            "SELECT COUNT(*) FROM managed_files WHERE parsed_key = ?1",
            [&parsed_key],
            |row| row.get(0),
        )?;
        if parsed_references == 0 {
            cleanup.parsed_keys.push(parsed_key);
        }
    }
    Ok(cleanup)
}

pub(super) fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    fn imported(hash: &str) -> ImportedObject {
        ImportedObject {
            content_hash: hash.into(),
            object_key: format!("objects/00/{hash}.pdf"),
            extension: "pdf".into(),
            size_bytes: 10,
        }
    }

    #[test]
    fn library_crud_persists_across_reopen() {
        let directory = tempdir().unwrap();
        let layout = StorageLayout::under(directory.path());
        let store = SqliteStore::open(&layout).unwrap();
        let library = store
            .create_reference_library("历年报告", FileCategory::Document)
            .unwrap();
        store
            .rename_reference_library(library.id, "优秀报告")
            .unwrap();
        drop(store);

        let reopened = SqliteStore::open(&layout).unwrap();
        let libraries = reopened.list_reference_libraries().unwrap();
        assert_eq!(libraries[0].library.name, "优秀报告");
    }

    #[test]
    fn version_two_results_are_invalidated_when_unit_metrics_are_added() {
        let directory = tempdir().unwrap();
        let database = directory.path().join("legacy.sqlite3");
        let connection = Connection::open(database).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE batches (
                     id INTEGER PRIMARY KEY,
                     name TEXT NOT NULL,
                     detection_status TEXT NOT NULL,
                     algorithm_id TEXT,
                     detection_error TEXT,
                     created_at INTEGER NOT NULL,
                     updated_at INTEGER NOT NULL
                 );
                 CREATE TABLE comparison_results (
                     id INTEGER PRIMARY KEY,
                     batch_id INTEGER NOT NULL,
                     query_file_id INTEGER NOT NULL,
                     source_file_id INTEGER NOT NULL,
                     scope_type TEXT NOT NULL,
                     scope_id INTEGER NOT NULL,
                     algorithm_id TEXT NOT NULL,
                     similarity REAL NOT NULL,
                     risk_regions_json TEXT NOT NULL,
                     created_at INTEGER NOT NULL,
                     UNIQUE(batch_id, query_file_id, source_file_id, scope_type, scope_id, algorithm_id)
                 );
                 INSERT INTO batches VALUES (1, '旧批次', 'ready', 'token-cosine', NULL, 0, 0);
                 INSERT INTO comparison_results VALUES (
                     1, 1, 1, 2, 'batch', 1, 'token-cosine', 0.5, '[]', 0
                 );
                 PRAGMA user_version = 2;",
            )
            .unwrap();

        migrate(&connection).unwrap();

        assert!(table_has_column(&connection, "comparison_results", "query_unit_count").unwrap());
        assert!(
            table_has_column(
                &connection,
                "comparison_results",
                "matched_unit_ranges_json"
            )
            .unwrap()
        );
        let result_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM comparison_results", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(result_count, 0);
        let status: (String, Option<String>) = connection
            .query_row(
                "SELECT detection_status, algorithm_id FROM batches WHERE id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, ("pending".into(), None));
    }

    #[test]
    fn version_three_results_are_invalidated_when_idf_evidence_is_added() {
        let directory = tempdir().unwrap();
        let database = directory.path().join("weighted-evidence.sqlite3");
        let connection = Connection::open(database).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE batches (
                     id INTEGER PRIMARY KEY,
                     name TEXT NOT NULL,
                     detection_status TEXT NOT NULL,
                     algorithm_id TEXT,
                     detection_error TEXT,
                     created_at INTEGER NOT NULL,
                     updated_at INTEGER NOT NULL
                 );
                 CREATE TABLE comparison_results (
                     id INTEGER PRIMARY KEY,
                     batch_id INTEGER NOT NULL,
                     query_file_id INTEGER NOT NULL,
                     source_file_id INTEGER NOT NULL,
                     scope_type TEXT NOT NULL,
                     scope_id INTEGER NOT NULL,
                     algorithm_id TEXT NOT NULL,
                     similarity REAL NOT NULL,
                     query_unit_count INTEGER NOT NULL,
                     matched_unit_count INTEGER NOT NULL,
                     matched_unit_ranges_json TEXT NOT NULL,
                     risk_regions_json TEXT NOT NULL,
                     created_at INTEGER NOT NULL,
                     UNIQUE(batch_id, query_file_id, source_file_id, scope_type, scope_id, algorithm_id)
                 );
                 INSERT INTO batches VALUES (1, '旧批次', 'ready', 'token-cosine', NULL, 0, 0);
                 INSERT INTO comparison_results VALUES (
                     1, 1, 1, 2, 'batch', 1, 'token-cosine', 0.2, 100, 20, '[]', '[]', 0
                 );
                 PRAGMA user_version = 3;",
            )
            .unwrap();

        migrate(&connection).unwrap();

        let result_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM comparison_results", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(result_count, 0);
        let status: (String, Option<String>) = connection
            .query_row(
                "SELECT detection_status, algorithm_id FROM batches WHERE id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, ("pending".into(), None));
        let version: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 4);
    }

    #[test]
    fn validates_names_and_rejects_case_insensitive_duplicates_per_category() {
        let directory = tempdir().unwrap();
        let store = SqliteStore::open(&StorageLayout::under(directory.path())).unwrap();
        assert!(
            store
                .create_reference_library("   ", FileCategory::Document)
                .is_err()
        );
        store
            .create_reference_library("Reports", FileCategory::Document)
            .unwrap();
        assert!(
            store
                .create_reference_library("reports", FileCategory::Document)
                .is_err()
        );
        assert!(
            store
                .create_reference_library("reports", FileCategory::Code)
                .is_ok()
        );
    }

    #[test]
    fn duplicate_content_is_skipped_inside_one_library() {
        let directory = tempdir().unwrap();
        let store = SqliteStore::open(&StorageLayout::under(directory.path())).unwrap();
        let library = store
            .create_reference_library("报告", FileCategory::Document)
            .unwrap();
        let new_file = || NewReferenceFile {
            library_id: library.id,
            name: "a.pdf".into(),
            relative_path: "a.pdf".into(),
            category: FileCategory::Document,
            object: imported("same"),
            parser_id: "liteparse-v2".into(),
            reusable_parsed_key: None,
        };

        assert!(matches!(
            store.insert_reference_file(new_file()).unwrap(),
            InsertReferenceFileOutcome::Inserted(_)
        ));
        assert!(matches!(
            store.insert_reference_file(new_file()).unwrap(),
            InsertReferenceFileOutcome::Duplicate(_)
        ));
    }

    #[test]
    fn deleting_last_reference_returns_cleanup_keys() {
        let directory = tempdir().unwrap();
        let store = SqliteStore::open(&StorageLayout::under(directory.path())).unwrap();
        let library = store
            .create_reference_library("报告", FileCategory::Document)
            .unwrap();
        let file = match store
            .insert_reference_file(NewReferenceFile {
                library_id: library.id,
                name: "a.pdf".into(),
                relative_path: "a.pdf".into(),
                category: FileCategory::Document,
                object: imported("cleanup"),
                parser_id: "liteparse-v2".into(),
                reusable_parsed_key: None,
            })
            .unwrap()
        {
            InsertReferenceFileOutcome::Inserted(file) => file,
            InsertReferenceFileOutcome::Duplicate(_) => unreachable!(),
        };

        let cleanup = store.delete_reference_file(library.id, file.id).unwrap();
        assert_eq!(cleanup.object_keys, vec!["objects/00/cleanup.pdf"]);
    }

    #[test]
    fn recovers_interrupted_parsing_as_retryable_failure() {
        let directory = tempdir().unwrap();
        let layout = StorageLayout::under(directory.path());
        let store = SqliteStore::open(&layout).unwrap();
        let library = store
            .create_reference_library("报告", FileCategory::Document)
            .unwrap();
        let file = match store
            .insert_reference_file(NewReferenceFile {
                library_id: library.id,
                name: "a.pdf".into(),
                relative_path: "a.pdf".into(),
                category: FileCategory::Document,
                object: imported("interrupted"),
                parser_id: "liteparse-v2".into(),
                reusable_parsed_key: None,
            })
            .unwrap()
        {
            InsertReferenceFileOutcome::Inserted(file) => file,
            InsertReferenceFileOutcome::Duplicate(_) => unreachable!(),
        };
        store
            .update_parse_status(file.id, ParseStatus::Parsing, "liteparse-v2", None, None)
            .unwrap();
        drop(store);

        let reopened = SqliteStore::open(&layout).unwrap();
        assert_eq!(reopened.recover_interrupted_parses().unwrap(), 1);
        let recovered = reopened.list_reference_files(library.id).unwrap().remove(0);
        assert_eq!(recovered.parse_status, ParseStatus::Failed);
        assert!(recovered.parse_error.unwrap().contains("请重试"));
    }
}
