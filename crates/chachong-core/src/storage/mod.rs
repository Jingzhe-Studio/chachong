mod batch_store;
mod object_store;
mod sqlite;

use std::{
    path::{Path, PathBuf},
    sync::PoisonError,
};

use thiserror::Error;

pub use batch_store::{
    BatchFileRecord, InsertBatchFileOutcome, NewBatchFile, NewComparison, ReferenceFileRecord,
    StoredComparison, StoredMatchSummary,
};
pub use object_store::{ImportedObject, ObjectStore};
pub use sqlite::{CleanupKeys, InsertReferenceFileOutcome, NewReferenceFile, SqliteStore};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageLayout {
    pub root: PathBuf,
    pub database: PathBuf,
    pub objects: PathBuf,
    pub parsed: PathBuf,
    pub indexes: PathBuf,
}

impl StorageLayout {
    pub fn under(root: impl AsRef<Path>) -> Self {
        let root = root.as_ref().to_path_buf();
        Self {
            database: root.join("chachong.sqlite3"),
            objects: root.join("objects"),
            parsed: root.join("parsed"),
            indexes: root.join("indexes"),
            root,
        }
    }

    pub fn ensure(&self) -> Result<(), StorageError> {
        for directory in [&self.root, &self.objects, &self.parsed, &self.indexes] {
            std::fs::create_dir_all(directory)?;
        }
        Ok(())
    }

    pub fn path_for_key(&self, key: &str) -> PathBuf {
        self.root.join(key)
    }
}

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("数据库错误：{0}")]
    Database(#[from] rusqlite::Error),
    #[error("文件系统错误：{0}")]
    Io(#[from] std::io::Error),
    #[error("序列化错误：{0}")]
    Serialization(#[from] serde_json::Error),
    #[error("存储状态无效：{0}")]
    InvalidState(String),
    #[error("{0}")]
    Validation(String),
}

impl<T> From<PoisonError<T>> for StorageError {
    fn from(_: PoisonError<T>) -> Self {
        Self::InvalidState("数据库连接锁已损坏".into())
    }
}
