use std::{
    fs,
    io::{BufReader, Read},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    domain::{FileCategory, FileId},
    parser::{LocationMapping, ParsedFile},
};

use super::{StorageError, StorageLayout};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedObject {
    pub content_hash: String,
    pub object_key: String,
    pub extension: String,
    pub size_bytes: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct ParsedArtifact {
    category: FileCategory,
    text: String,
    locations: Vec<LocationMapping>,
}

#[derive(Debug, Clone)]
pub struct ObjectStore {
    layout: StorageLayout,
}

impl ObjectStore {
    pub fn new(layout: StorageLayout) -> Result<Self, StorageError> {
        layout.ensure()?;
        Ok(Self { layout })
    }

    pub fn layout(&self) -> &StorageLayout {
        &self.layout
    }

    pub fn import(&self, source: &Path) -> Result<ImportedObject, StorageError> {
        let metadata = fs::metadata(source)?;
        let content_hash = hash_file(source)?;
        let extension = normalized_extension(source);
        let file_name = if extension.is_empty() {
            content_hash.clone()
        } else {
            format!("{content_hash}.{extension}")
        };
        let object_key = PathBuf::from("objects")
            .join(&content_hash[..2])
            .join(file_name);
        let destination = self.layout.path_for_key(&object_key.to_string_lossy());

        if !destination.exists() {
            let parent = destination
                .parent()
                .ok_or_else(|| StorageError::InvalidState("托管文件缺少父目录".into()))?;
            fs::create_dir_all(parent)?;
            let temporary = temporary_sibling(&destination);
            fs::copy(source, &temporary)?;
            match fs::rename(&temporary, &destination) {
                Ok(()) => {}
                Err(_) if destination.exists() => {
                    let _ = fs::remove_file(temporary);
                }
                Err(error) => return Err(error.into()),
            }
        }

        Ok(ImportedObject {
            content_hash,
            object_key: key_string(&object_key),
            extension,
            size_bytes: metadata.len(),
        })
    }

    pub fn object_path(&self, object_key: &str) -> PathBuf {
        self.layout.path_for_key(object_key)
    }

    pub fn parsed_key(&self, parser_id: &str, content_hash: &str) -> String {
        key_string(
            &PathBuf::from("parsed")
                .join(parser_id)
                .join(format!("{content_hash}.json")),
        )
    }

    pub fn parsed_exists(&self, parsed_key: &str) -> bool {
        self.layout.path_for_key(parsed_key).is_file()
    }

    pub async fn write_parsed(
        &self,
        parsed_key: &str,
        parsed: &ParsedFile,
    ) -> Result<(), StorageError> {
        let artifact = ParsedArtifact {
            category: parsed.category,
            text: parsed.text.clone(),
            locations: parsed.locations.clone(),
        };
        let bytes = serde_json::to_vec(&artifact)?;
        let destination = self.layout.path_for_key(parsed_key);
        let parent = destination
            .parent()
            .ok_or_else(|| StorageError::InvalidState("解析产物缺少父目录".into()))?;
        tokio::fs::create_dir_all(parent).await?;
        let temporary = temporary_sibling(&destination);
        tokio::fs::write(&temporary, bytes).await?;
        match tokio::fs::rename(&temporary, &destination).await {
            Ok(()) => Ok(()),
            Err(_) if destination.exists() => {
                let _ = tokio::fs::remove_file(temporary).await;
                Ok(())
            }
            Err(error) => Err(error.into()),
        }
    }

    pub async fn read_parsed(
        &self,
        parsed_key: &str,
        file_id: FileId,
    ) -> Result<ParsedFile, StorageError> {
        let bytes = tokio::fs::read(self.layout.path_for_key(parsed_key)).await?;
        let artifact: ParsedArtifact = serde_json::from_slice(&bytes)?;
        Ok(ParsedFile {
            file_id,
            category: artifact.category,
            text: artifact.text,
            locations: artifact.locations,
        })
    }

    pub fn remove_keys(&self, keys: impl IntoIterator<Item = String>) {
        for key in keys {
            let _ = fs::remove_file(self.layout.path_for_key(&key));
        }
    }
}

fn hash_file(path: &Path) -> Result<String, StorageError> {
    let file = fs::File::open(path)?;
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];

    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }

    Ok(format!("{:x}", hasher.finalize()))
}

fn normalized_extension(path: &Path) -> String {
    path.extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn temporary_sibling(destination: &Path) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("artifact");
    destination.with_file_name(format!(".{name}.tmp-{}-{nonce}", std::process::id()))
}

fn key_string(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn identical_files_share_an_object_key() {
        let directory = tempdir().unwrap();
        let store = ObjectStore::new(StorageLayout::under(directory.path())).unwrap();
        let first = directory.path().join("first.pdf");
        let second = directory.path().join("second.pdf");
        fs::write(&first, b"same").unwrap();
        fs::write(&second, b"same").unwrap();

        let first_object = store.import(&first).unwrap();
        let second_object = store.import(&second).unwrap();

        assert_eq!(first_object.object_key, second_object.object_key);
        assert!(store.object_path(&first_object.object_key).is_file());
    }
}
