use crate::{
    algorithms::features::{
        WINNOWING_KIND, build_chunk_index, compare_feature_sequences, decode, prepare,
        winnowing_features,
    },
    detection::{
        AlgorithmDescriptor, Comparator, ComparisonEvidence, DetectionAlgorithm, DetectionError,
        PreparedFile, Preprocessor, RetrievalIndex, Retriever,
    },
    domain::FileCategory,
    parser::ParsedFile,
};

static SUPPORTED_CATEGORIES: [FileCategory; 2] = [FileCategory::Document, FileCategory::Code];

#[derive(Default)]
pub struct WinnowingAlgorithm {
    preprocessor: WinnowingPreprocessor,
    retriever: FingerprintRetriever,
    comparator: FingerprintComparator,
}

impl DetectionAlgorithm for WinnowingAlgorithm {
    fn descriptor(&self) -> AlgorithmDescriptor {
        AlgorithmDescriptor {
            id: "winnowing-fingerprint",
            display_name: "稀疏指纹覆盖率",
            description: "按文本块召回稀疏指纹，再计算当前文件中连续指纹链的覆盖比例。",
            supported_categories: &SUPPORTED_CATEGORIES,
        }
    }

    fn preprocessor(&self) -> &dyn Preprocessor {
        &self.preprocessor
    }

    fn retriever(&self) -> &dyn Retriever {
        &self.retriever
    }

    fn comparator(&self) -> &dyn Comparator {
        &self.comparator
    }
}

#[derive(Default)]
struct WinnowingPreprocessor;

impl Preprocessor for WinnowingPreprocessor {
    fn preprocess(&self, file: &ParsedFile) -> Result<PreparedFile, DetectionError> {
        prepare(file, WINNOWING_KIND, winnowing_features(file, 5, 4))
    }
}

#[derive(Default)]
struct FingerprintRetriever;

impl Retriever for FingerprintRetriever {
    fn build_index(
        &self,
        corpus: &[PreparedFile],
    ) -> Result<Box<dyn RetrievalIndex>, DetectionError> {
        build_chunk_index(corpus, WINNOWING_KIND)
    }
}

#[derive(Default)]
struct FingerprintComparator;

impl Comparator for FingerprintComparator {
    fn compare(
        &self,
        query: &PreparedFile,
        source: &PreparedFile,
    ) -> Result<ComparisonEvidence, DetectionError> {
        let query = decode(query, WINNOWING_KIND)?;
        let source = decode(source, WINNOWING_KIND)?;
        let (similarity, risk_regions) =
            compare_feature_sequences(&query.features, &source.features, 2);
        Ok(ComparisonEvidence {
            similarity,
            risk_regions,
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::{detection::DetectionPipeline, domain::FileId, parser::ParsedFile};

    use super::*;

    #[test]
    fn fingerprint_index_returns_shared_sequence() {
        let algorithm = WinnowingAlgorithm::default();
        let pipeline = DetectionPipeline::new(&algorithm);
        let first = parsed(1, "fn calculate value total result return total value");
        let second = parsed(
            2,
            "fn calculate value total result return total value extra",
        );
        let third = parsed(3, "class database connection open close commit rollback");
        let prepared: Vec<_> = [&first, &second, &third]
            .into_iter()
            .map(|file| pipeline.preprocess(file).unwrap())
            .collect();
        let index = pipeline.build_index(&prepared).unwrap();
        let candidates = index.retrieve(&prepared[0], 3).unwrap();
        assert!(
            candidates
                .iter()
                .any(|candidate| candidate.file_id == FileId(2))
        );
        assert!(
            pipeline
                .compare(&prepared[0], &prepared[1])
                .unwrap()
                .similarity
                > 0.5
        );
    }

    fn parsed(id: u64, text: &str) -> ParsedFile {
        ParsedFile {
            file_id: FileId(id),
            category: FileCategory::Code,
            text: text.into(),
            locations: Vec::new(),
        }
    }
}
