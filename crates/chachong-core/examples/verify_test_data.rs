use std::{collections::HashMap, env, error::Error, path::PathBuf};

use chachong_core::{
    application::{AppCore, FileMatchSummary},
    domain::{FileCategory, FileId},
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let dataset = env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("output/pdf/chachong-test-data"))
        .canonicalize()?;
    let temporary = tempfile::tempdir()?;
    let core = AppCore::open(temporary.path().join("app-data"))?;

    let documents = core
        .references()
        .create_library("fixture-documents", FileCategory::Document)?;
    let code = core
        .references()
        .create_library("fixture-code", FileCategory::Code)?;
    let document_import = core
        .references()
        .import_paths(
            documents.id,
            vec![dataset.join("reference_library/documents")],
            |_| {},
        )
        .await?;
    let code_import = core
        .references()
        .import_paths(
            code.id,
            vec![dataset.join("reference_library/code")],
            |_| {},
        )
        .await?;
    assert_eq!(document_import.ready, 2, "both reference PDFs must parse");
    assert_eq!(code_import.ready, 2, "both reference code files must parse");

    let batch_import = core
        .batches()
        .import_batch(dataset.join("batch"), |_| {})
        .await?;
    assert_eq!(batch_import.batch.work_item_count, 4);
    assert_eq!(batch_import.ready, 8, "all assignment files must parse");

    let mut query_ids = HashMap::new();
    for item in core.batches().list_work_items(batch_import.batch.id)? {
        for view in core.batches().list_work_item_files(item.id)? {
            query_ids.insert(view.file.relative_path.clone(), view.file.id);
        }
    }

    for descriptor in core.algorithms().descriptors() {
        let run = core
            .batches()
            .run_detection(
                batch_import.batch.id,
                core.algorithms().resolve(descriptor.id).unwrap(),
                |_| {},
            )
            .await?;
        assert!(
            run.compared_pairs < 40,
            "{}: block retrieval should prune unrelated file pairs",
            descriptor.id
        );

        let exact_document = matches_for(
            &core,
            batch_import.batch.id,
            &query_ids,
            "student_01_exact_copy/report.pdf",
        )?;
        assert_match(
            descriptor.id,
            &exact_document,
            "reference",
            "arxiv_1604.05171v1_blood_flow.pdf",
            0.999,
        );

        let exact_code = matches_for(
            &core,
            batch_import.batch.id,
            &query_ids,
            "student_01_exact_copy/src/merge_sort.py",
        )?;
        assert_match(
            descriptor.id,
            &exact_code,
            "reference",
            "merge_sort.py",
            0.999,
        );

        let partial_document = matches_for(
            &core,
            batch_import.batch.id,
            &query_ids,
            "student_02_partial_and_modified/report.pdf",
        )?;
        assert_match(
            descriptor.id,
            &partial_document,
            "reference",
            "arxiv_1604.05177v1_jugular_pulse.pdf",
            0.15,
        );

        let adapted_code = matches_for(
            &core,
            batch_import.batch.id,
            &query_ids,
            "student_02_partial_and_modified/src/binary_search_adapted.py",
        )?;
        assert_match(
            descriptor.id,
            &adapted_code,
            "reference",
            "binary_search.py",
            0.15,
        );

        let peer_copy = matches_for(
            &core,
            batch_import.batch.id,
            &query_ids,
            "student_02_partial_and_modified/src/peer_shared.py",
        )?;
        assert_match(
            descriptor.id,
            &peer_copy,
            "batch",
            "peer_shared_copy.py",
            0.999,
        );

        println!(
            "{}: {} files, {} comparisons, {} stored matches",
            descriptor.id, run.query_files, run.compared_pairs, run.matches
        );
        let document_control = matches_for(
            &core,
            batch_import.batch.id,
            &query_ids,
            "student_03_peer_copy/report.pdf",
        )?;
        let code_control = matches_for(
            &core,
            batch_import.batch.id,
            &query_ids,
            "student_04_clean_code/euclidean_distance.py",
        )?;
        assert_no_reference_match(descriptor.id, "document control", &document_control);
        assert_no_reference_match(descriptor.id, "code control", &code_control);
        print_optional_control("document control", &document_control);
        print_optional_control("code control", &code_control);
    }

    println!("fixture verification passed: {}", dataset.display());
    Ok(())
}

fn matches_for(
    core: &AppCore,
    batch_id: chachong_core::domain::BatchId,
    query_ids: &HashMap<String, FileId>,
    relative_path: &str,
) -> Result<Vec<FileMatchSummary>, Box<dyn Error>> {
    let file_id = query_ids
        .get(relative_path)
        .unwrap_or_else(|| panic!("missing query fixture: {relative_path}"));
    Ok(core.batches().list_file_matches(batch_id, *file_id)?)
}

fn assert_match(
    algorithm: &str,
    matches: &[FileMatchSummary],
    source_kind: &str,
    source_name: &str,
    minimum_similarity: f32,
) {
    let matched = matches.iter().find(|candidate| {
        candidate.source_kind == source_kind && candidate.source_name == source_name
    });
    let matched = matched
        .unwrap_or_else(|| panic!("{algorithm}: expected {source_kind} match from {source_name}"));
    assert!(
        matched.similarity >= minimum_similarity,
        "{algorithm}: {} similarity {} was below {}",
        source_name,
        matched.similarity,
        minimum_similarity
    );
    assert!(
        matched.risk_count > 0,
        "match must include highlighted regions"
    );
}

fn print_optional_control(label: &str, matches: &[FileMatchSummary]) {
    let reference = matches
        .iter()
        .filter(|item| item.source_kind == "reference")
        .map(|item| format!("{}={:.3}", item.source_name, item.similarity))
        .collect::<Vec<_>>();
    if reference.is_empty() {
        println!("  {label}: no stored reference match");
    } else {
        println!("  {label}: {}", reference.join(", "));
    }
}

fn assert_no_reference_match(algorithm: &str, label: &str, matches: &[FileMatchSummary]) {
    assert!(
        matches.iter().all(|item| item.source_kind != "reference"),
        "{algorithm}: unrelated {label} should not match the reference library"
    );
}
