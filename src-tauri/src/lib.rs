use std::path::PathBuf;

use chachong_core::{
    application::{
        AppCore, BatchFileProgress, BatchImportSummary, DetectionRunSummary, FileMatchSummary,
        MatchDetail, ReferenceFileProgress, ReferenceImportSummary, WorkItemFileView,
        WorkItemSimilarityOverview,
    },
    domain::{
        BatchId, BatchSummary, FileCategory, FileId, ManagedFile, ReferenceLibrary,
        ReferenceLibraryId, ReferenceLibrarySummary, WorkItemId, WorkItemSummary,
    },
};
use serde::Serialize;
use tauri::{Emitter, Manager};

#[derive(Default)]
struct CorpusOperation(tokio::sync::Mutex<()>);

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AlgorithmInfo {
    id: &'static str,
    display_name: &'static str,
    description: &'static str,
    supported_categories: &'static [FileCategory],
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AppInfo {
    name: &'static str,
    algorithms: Vec<AlgorithmInfo>,
}

#[tauri::command]
fn app_info(core: tauri::State<'_, AppCore>) -> AppInfo {
    let algorithms = core
        .algorithms()
        .descriptors()
        .into_iter()
        .map(|descriptor| AlgorithmInfo {
            id: descriptor.id,
            display_name: descriptor.display_name,
            description: descriptor.description,
            supported_categories: descriptor.supported_categories,
        })
        .collect();

    AppInfo {
        name: "查重工作台",
        algorithms,
    }
}

#[tauri::command]
fn list_batches(core: tauri::State<'_, AppCore>) -> Result<Vec<BatchSummary>, String> {
    core.batches()
        .list_batches()
        .map_err(|error| error.to_string())
}

#[tauri::command(rename_all = "camelCase")]
async fn import_batch(
    app: tauri::AppHandle,
    core: tauri::State<'_, AppCore>,
    path: String,
) -> Result<BatchImportSummary, String> {
    core.batches()
        .import_batch(PathBuf::from(path), move |progress| {
            let _ = app.emit("batch-file-status", progress.clone());
        })
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command(rename_all = "camelCase")]
fn rename_batch(
    core: tauri::State<'_, AppCore>,
    batch_id: u64,
    name: String,
) -> Result<BatchSummary, String> {
    core.batches()
        .rename_batch(BatchId(batch_id), &name)
        .map_err(|error| error.to_string())
}

#[tauri::command(rename_all = "camelCase")]
async fn delete_batch(
    core: tauri::State<'_, AppCore>,
    operation: tauri::State<'_, CorpusOperation>,
    batch_id: u64,
) -> Result<(), String> {
    let _guard = operation.0.lock().await;
    core.batches()
        .delete_batch(BatchId(batch_id))
        .map_err(|error| error.to_string())
}

#[tauri::command(rename_all = "camelCase")]
fn list_work_items(
    core: tauri::State<'_, AppCore>,
    batch_id: u64,
) -> Result<Vec<WorkItemSummary>, String> {
    core.batches()
        .list_work_items(BatchId(batch_id))
        .map_err(|error| error.to_string())
}

#[tauri::command(rename_all = "camelCase")]
fn get_work_item(
    core: tauri::State<'_, AppCore>,
    work_item_id: u64,
) -> Result<WorkItemSummary, String> {
    core.batches()
        .get_work_item(WorkItemId(work_item_id))
        .map_err(|error| error.to_string())
}

#[tauri::command(rename_all = "camelCase")]
fn list_work_item_files(
    core: tauri::State<'_, AppCore>,
    work_item_id: u64,
) -> Result<Vec<WorkItemFileView>, String> {
    core.batches()
        .list_work_item_files(WorkItemId(work_item_id))
        .map_err(|error| error.to_string())
}

#[tauri::command(rename_all = "camelCase")]
fn get_work_item_similarity_overview(
    core: tauri::State<'_, AppCore>,
    work_item_id: u64,
) -> Result<WorkItemSimilarityOverview, String> {
    core.batches()
        .get_work_item_similarity_overview(WorkItemId(work_item_id))
        .map_err(|error| error.to_string())
}

#[tauri::command(rename_all = "camelCase")]
async fn retry_batch_parse(
    app: tauri::AppHandle,
    core: tauri::State<'_, AppCore>,
    operation: tauri::State<'_, CorpusOperation>,
    work_item_id: u64,
    file_id: u64,
) -> Result<ManagedFile, String> {
    let _guard = operation.0.lock().await;
    let work_item_id = WorkItemId(work_item_id);
    let item = core
        .batches()
        .get_work_item(work_item_id)
        .map_err(|error| error.to_string())?;
    let file = core
        .batches()
        .retry_file(work_item_id, FileId(file_id))
        .await
        .map_err(|error| error.to_string())?;
    let _ = app.emit(
        "batch-file-status",
        BatchFileProgress {
            batch_id: item.batch_id,
            work_item_id,
            file: file.clone(),
        },
    );
    Ok(file)
}

#[tauri::command(rename_all = "camelCase")]
async fn run_batch_detection(
    app: tauri::AppHandle,
    core: tauri::State<'_, AppCore>,
    operation: tauri::State<'_, CorpusOperation>,
    batch_id: u64,
    algorithm_id: String,
) -> Result<DetectionRunSummary, String> {
    let _guard = operation.0.lock().await;
    let algorithm = core
        .algorithms()
        .resolve(&algorithm_id)
        .ok_or_else(|| format!("未知查重算法：{algorithm_id}"))?;
    core.batches()
        .run_detection(BatchId(batch_id), algorithm, move |progress| {
            let _ = app.emit("detection-progress", progress.clone());
        })
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command(rename_all = "camelCase")]
fn list_file_matches(
    core: tauri::State<'_, AppCore>,
    batch_id: u64,
    file_id: u64,
) -> Result<Vec<FileMatchSummary>, String> {
    core.batches()
        .list_file_matches(BatchId(batch_id), FileId(file_id))
        .map_err(|error| error.to_string())
}

#[tauri::command(rename_all = "camelCase")]
async fn get_match_detail(
    core: tauri::State<'_, AppCore>,
    query_file_id: u64,
    result_id: u64,
) -> Result<MatchDetail, String> {
    core.batches()
        .get_match_detail(FileId(query_file_id), result_id)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn list_reference_libraries(
    core: tauri::State<'_, AppCore>,
) -> Result<Vec<ReferenceLibrarySummary>, String> {
    core.references()
        .list_libraries()
        .map_err(|error| error.to_string())
}

#[tauri::command(rename_all = "camelCase")]
fn create_reference_library(
    core: tauri::State<'_, AppCore>,
    name: String,
    category: FileCategory,
) -> Result<ReferenceLibrary, String> {
    core.references()
        .create_library(&name, category)
        .map_err(|error| error.to_string())
}

#[tauri::command(rename_all = "camelCase")]
fn rename_reference_library(
    core: tauri::State<'_, AppCore>,
    library_id: u64,
    name: String,
) -> Result<ReferenceLibrary, String> {
    core.references()
        .rename_library(ReferenceLibraryId(library_id), &name)
        .map_err(|error| error.to_string())
}

#[tauri::command(rename_all = "camelCase")]
async fn delete_reference_library(
    core: tauri::State<'_, AppCore>,
    operation: tauri::State<'_, CorpusOperation>,
    library_id: u64,
) -> Result<(), String> {
    let _guard = operation.0.lock().await;
    core.references()
        .delete_library(ReferenceLibraryId(library_id))
        .map_err(|error| error.to_string())
}

#[tauri::command(rename_all = "camelCase")]
fn list_reference_files(
    core: tauri::State<'_, AppCore>,
    library_id: u64,
) -> Result<Vec<ManagedFile>, String> {
    core.references()
        .list_files(ReferenceLibraryId(library_id))
        .map_err(|error| error.to_string())
}

#[tauri::command(rename_all = "camelCase")]
async fn import_reference_paths(
    app: tauri::AppHandle,
    core: tauri::State<'_, AppCore>,
    operation: tauri::State<'_, CorpusOperation>,
    library_id: u64,
    paths: Vec<String>,
) -> Result<ReferenceImportSummary, String> {
    let _guard = operation.0.lock().await;
    let library_id = ReferenceLibraryId(library_id);
    let paths = paths.into_iter().map(PathBuf::from).collect();
    core.references()
        .import_paths(library_id, paths, move |progress| {
            let _ = app.emit("reference-file-status", progress.clone());
        })
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command(rename_all = "camelCase")]
async fn delete_reference_file(
    core: tauri::State<'_, AppCore>,
    operation: tauri::State<'_, CorpusOperation>,
    library_id: u64,
    file_id: u64,
) -> Result<(), String> {
    let _guard = operation.0.lock().await;
    core.references()
        .delete_file(ReferenceLibraryId(library_id), FileId(file_id))
        .map_err(|error| error.to_string())
}

#[tauri::command(rename_all = "camelCase")]
async fn retry_reference_parse(
    app: tauri::AppHandle,
    core: tauri::State<'_, AppCore>,
    operation: tauri::State<'_, CorpusOperation>,
    library_id: u64,
    file_id: u64,
) -> Result<ManagedFile, String> {
    let _guard = operation.0.lock().await;
    let library_id = ReferenceLibraryId(library_id);
    let file = core
        .references()
        .retry_file(library_id, FileId(file_id))
        .await
        .map_err(|error| error.to_string())?;
    let _ = app.emit(
        "reference-file-status",
        ReferenceFileProgress {
            library_id,
            file: file.clone(),
        },
    );
    Ok(file)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let data_root = app.path().app_data_dir()?;
            app.manage(AppCore::open(data_root)?);
            app.manage(CorpusOperation::default());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            app_info,
            list_batches,
            import_batch,
            rename_batch,
            delete_batch,
            list_work_items,
            get_work_item,
            list_work_item_files,
            get_work_item_similarity_overview,
            retry_batch_parse,
            run_batch_detection,
            list_file_matches,
            get_match_detail,
            list_reference_libraries,
            create_reference_library,
            rename_reference_library,
            delete_reference_library,
            list_reference_files,
            import_reference_paths,
            delete_reference_file,
            retry_reference_parse,
        ])
        .run(tauri::generate_context!())
        .expect("failed to run the desktop application");
}

#[cfg(test)]
mod tests {
    use chachong_core::domain::ParseStatus;

    use super::*;

    #[test]
    fn progress_event_payload_uses_frontend_field_names() {
        let progress = ReferenceFileProgress {
            library_id: ReferenceLibraryId(7),
            file: ManagedFile {
                id: FileId(11),
                name: "broken.pdf".into(),
                relative_path: "samples/broken.pdf".into(),
                object_key: "objects/00/hash.pdf".into(),
                parsed_key: None,
                content_hash: "hash".into(),
                extension: "pdf".into(),
                size_bytes: 42,
                category: FileCategory::Document,
                parse_status: ParseStatus::Failed,
                parse_error: Some("解析失败".into()),
                parser_id: Some("liteparse-v2".into()),
            },
        };

        let value = serde_json::to_value(progress).unwrap();
        assert_eq!(value["libraryId"], 7);
        assert_eq!(value["file"]["parseStatus"], "failed");
        assert!(value.get("library_id").is_none());
    }
}
