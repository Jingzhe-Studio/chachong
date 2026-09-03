use std::{
    collections::BTreeSet,
    error::Error,
    fmt,
    fs::File,
    io::Read,
    path::{Path, PathBuf},
};

use walkdir::{DirEntry, WalkDir};

use crate::domain::{FileCategory, ReferenceLibraryId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BatchImportSource {
    RootDirectory(PathBuf),
    Archive(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredFile {
    pub source_path: PathBuf,
    pub relative_path: PathBuf,
    pub category: FileCategory,
}

/// A first-level directory discovered as one work item during import preview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredWorkItem {
    pub name: String,
    pub files: Vec<DiscoveredFile>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchImportPreview {
    pub batch_name: String,
    pub work_items: Vec<DiscoveredWorkItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReferenceImportRequest {
    pub library_id: ReferenceLibraryId,
    pub paths: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportError {
    message: String,
}

impl ImportError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for ImportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for ImportError {}

pub trait BatchImportPlanner: Send + Sync {
    fn preview(&self, source: &BatchImportSource) -> Result<BatchImportPreview, ImportError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReferenceImportCandidate {
    pub source_path: PathBuf,
    pub relative_path: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReferenceImportSkip {
    pub path: String,
    pub reason: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReferenceImportDiscovery {
    pub candidates: Vec<ReferenceImportCandidate>,
    pub skipped: Vec<ReferenceImportSkip>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchFileCandidate {
    pub source_path: PathBuf,
    pub relative_path: String,
    pub name: String,
    pub category: FileCategory,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchWorkItemCandidate {
    pub name: String,
    pub files: Vec<BatchFileCandidate>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BatchImportDiscovery {
    pub batch_name: String,
    pub work_items: Vec<BatchWorkItemCandidate>,
    pub skipped: Vec<ReferenceImportSkip>,
}

pub fn discover_batch(root: &Path) -> Result<BatchImportDiscovery, ImportError> {
    if !root.is_dir() {
        return Err(ImportError::new("批次导入路径必须是目录"));
    }
    let batch_name = root
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or("未命名批次")
        .to_owned();
    let mut discovery = BatchImportDiscovery {
        batch_name,
        ..BatchImportDiscovery::default()
    };
    let mut entries: Vec<_> = std::fs::read_dir(root)
        .map_err(|error| ImportError::new(format!("无法读取批次目录：{error}")))?
        .filter_map(Result::ok)
        .collect();
    entries.sort_by_key(|entry| entry.file_name());

    for entry in entries {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            discovery.skipped.push(ReferenceImportSkip {
                path: display_path(&path),
                reason: "无法读取文件类型".into(),
            });
            continue;
        };
        if file_type.is_symlink() {
            discovery.skipped.push(ReferenceImportSkip {
                path: display_path(&path),
                reason: "已跳过符号链接".into(),
            });
            continue;
        }
        if file_type.is_dir() {
            if is_ignored_directory_name(&entry.file_name().to_string_lossy()) {
                continue;
            }
            let mut item = BatchWorkItemCandidate {
                name: entry.file_name().to_string_lossy().into_owned(),
                files: Vec::new(),
            };
            discover_work_item_files(root, &path, &mut item, &mut discovery.skipped);
            if item.files.is_empty() {
                discovery.skipped.push(ReferenceImportSkip {
                    path: display_path(&path),
                    reason: "作业项中没有可导入的文档或代码文件".into(),
                });
            } else {
                discovery.work_items.push(item);
            }
        } else if file_type.is_file() {
            let mut item = BatchWorkItemCandidate {
                name: path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("未命名作业")
                    .to_owned(),
                files: Vec::new(),
            };
            if let Some(file) = classify_batch_file(root, &path, &mut discovery.skipped) {
                item.files.push(file);
                discovery.work_items.push(item);
            }
        }
    }

    if discovery.work_items.is_empty() {
        return Err(ImportError::new("批次目录中没有可导入的作业文件"));
    }
    Ok(discovery)
}

fn discover_work_item_files(
    root: &Path,
    item_root: &Path,
    item: &mut BatchWorkItemCandidate,
    skipped: &mut Vec<ReferenceImportSkip>,
) {
    for entry in WalkDir::new(item_root)
        .follow_links(false)
        .into_iter()
        .filter_entry(allowed_directory)
    {
        match entry {
            Ok(entry) if entry.file_type().is_file() && !entry.path_is_symlink() => {
                if let Some(file) = classify_batch_file(root, entry.path(), skipped) {
                    item.files.push(file);
                }
            }
            Ok(entry) if entry.path_is_symlink() => skipped.push(ReferenceImportSkip {
                path: display_path(entry.path()),
                reason: "已跳过符号链接".into(),
            }),
            Ok(_) => {}
            Err(error) => skipped.push(ReferenceImportSkip {
                path: error
                    .path()
                    .map(display_path)
                    .unwrap_or_else(|| display_path(item_root)),
                reason: format!("无法读取：{error}"),
            }),
        }
    }
    item.files
        .sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
}

fn classify_batch_file(
    root: &Path,
    path: &Path,
    skipped: &mut Vec<ReferenceImportSkip>,
) -> Option<BatchFileCandidate> {
    let category = if is_supported_document(path) {
        FileCategory::Document
    } else if is_probably_binary(path) {
        skipped.push(ReferenceImportSkip {
            path: display_path(path),
            reason: "已跳过二进制文件".into(),
        });
        return None;
    } else {
        FileCategory::Code
    };
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("unnamed")
        .to_owned();
    Some(BatchFileCandidate {
        source_path: path.to_owned(),
        relative_path: path
            .strip_prefix(root)
            .map(display_path)
            .unwrap_or_else(|_| name.clone()),
        name,
        category,
    })
}

pub fn discover_reference_files(
    category: FileCategory,
    selections: &[PathBuf],
) -> ReferenceImportDiscovery {
    let mut discovery = ReferenceImportDiscovery::default();
    let mut seen = BTreeSet::new();

    for selection in selections {
        if selection.is_file() {
            add_candidate(
                category,
                selection,
                selection.file_name().map(PathBuf::from),
                &mut seen,
                &mut discovery,
            );
        } else if selection.is_dir() {
            let root_name = selection.file_name().map(PathBuf::from).unwrap_or_default();
            for entry in WalkDir::new(selection)
                .follow_links(false)
                .into_iter()
                .filter_entry(allowed_directory)
            {
                match entry {
                    Ok(entry) if entry.file_type().is_file() && !entry.path_is_symlink() => {
                        let relative = entry
                            .path()
                            .strip_prefix(selection)
                            .map(|path| root_name.join(path))
                            .ok();
                        add_candidate(category, entry.path(), relative, &mut seen, &mut discovery);
                    }
                    Ok(_) => {}
                    Err(error) => discovery.skipped.push(ReferenceImportSkip {
                        path: error
                            .path()
                            .map(display_path)
                            .unwrap_or_else(|| display_path(selection)),
                        reason: format!("无法读取：{error}"),
                    }),
                }
            }
        } else {
            discovery.skipped.push(ReferenceImportSkip {
                path: display_path(selection),
                reason: "路径不存在或不是普通文件/目录".into(),
            });
        }
    }

    discovery
}

fn add_candidate(
    category: FileCategory,
    source: &Path,
    relative: Option<PathBuf>,
    seen: &mut BTreeSet<PathBuf>,
    discovery: &mut ReferenceImportDiscovery,
) {
    let canonical = source
        .canonicalize()
        .unwrap_or_else(|_| source.to_path_buf());
    if !seen.insert(canonical) {
        return;
    }

    if category == FileCategory::Document && !is_supported_document(source) {
        discovery.skipped.push(ReferenceImportSkip {
            path: display_path(source),
            reason: "文档库仅支持 PDF、DOC 和 DOCX".into(),
        });
        return;
    }

    if category == FileCategory::Code && is_probably_binary(source) {
        discovery.skipped.push(ReferenceImportSkip {
            path: display_path(source),
            reason: "已跳过二进制文件".into(),
        });
        return;
    }

    let name = source
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("unnamed")
        .to_owned();
    discovery.candidates.push(ReferenceImportCandidate {
        source_path: source.to_path_buf(),
        relative_path: relative
            .as_deref()
            .map(display_path)
            .unwrap_or_else(|| name.clone()),
        name,
    });
}

fn allowed_directory(entry: &DirEntry) -> bool {
    if !entry.file_type().is_dir() {
        return true;
    }
    !is_ignored_directory_name(&entry.file_name().to_string_lossy())
}

fn is_ignored_directory_name(name: &str) -> bool {
    matches!(name, ".git" | "node_modules" | "target" | "dist" | "build")
}

fn is_supported_document(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("pdf" | "doc" | "docx")
    )
}

fn is_probably_binary(path: &Path) -> bool {
    let Ok(mut file) = File::open(path) else {
        return false;
    };
    let mut buffer = [0_u8; 8192];
    let Ok(read) = file.read(&mut buffer) else {
        return false;
    };
    buffer[..read].contains(&0)
}

fn display_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn recursively_discovers_code_and_ignores_build_directories() {
        let directory = tempdir().unwrap();
        fs::create_dir_all(directory.path().join("src")).unwrap();
        fs::create_dir_all(directory.path().join("target")).unwrap();
        fs::write(directory.path().join("src/main.rs"), "fn main() {}\n").unwrap();
        fs::write(directory.path().join("target/generated.rs"), "generated").unwrap();

        let result = discover_reference_files(FileCategory::Code, &[directory.path().into()]);

        assert_eq!(result.candidates.len(), 1);
        assert!(result.candidates[0].relative_path.ends_with("src/main.rs"));
    }

    #[test]
    fn skips_unsupported_documents_and_binary_code() {
        let directory = tempdir().unwrap();
        let text = directory.path().join("notes.txt");
        let binary = directory.path().join("binary.bin");
        fs::write(&text, "notes").unwrap();
        fs::write(&binary, [0, 1, 2]).unwrap();

        let documents = discover_reference_files(FileCategory::Document, &[text]);
        let code = discover_reference_files(FileCategory::Code, &[binary]);

        assert_eq!(documents.skipped.len(), 1);
        assert_eq!(code.skipped.len(), 1);
    }

    #[test]
    fn discovers_first_level_directories_as_multi_file_work_items() {
        let directory = tempdir().unwrap();
        fs::create_dir_all(directory.path().join("张三/src")).unwrap();
        fs::create_dir_all(directory.path().join("李四")).unwrap();
        fs::write(directory.path().join("张三/report.pdf"), b"pdf").unwrap();
        fs::write(directory.path().join("张三/src/main.rs"), "fn main() {}\n").unwrap();
        fs::write(directory.path().join("李四/main.py"), "print('ok')\n").unwrap();

        let result = discover_batch(directory.path()).unwrap();

        assert_eq!(result.work_items.len(), 2);
        assert_eq!(result.work_items[0].name, "张三");
        assert_eq!(result.work_items[0].files.len(), 2);
        assert!(
            result.work_items[0]
                .files
                .iter()
                .any(|file| file.category == FileCategory::Document)
        );
        assert!(
            result.work_items[0]
                .files
                .iter()
                .any(|file| file.category == FileCategory::Code)
        );
    }
}
