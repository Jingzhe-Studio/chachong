use crate::{
    algorithms::features::{
        TOKEN_KIND, build_chunk_index, compare_feature_sequences, prepare, token_features,
    },
    detection::{
        AlgorithmDescriptor, Comparator, ComparisonEvidence, DetectionAlgorithm, DetectionError,
        FeatureWeightProvider, PreparedFile, Preprocessor, RetrievalIndex, Retriever,
    },
    domain::FileCategory,
    parser::ParsedFile,
};

static SUPPORTED_CATEGORIES: [FileCategory; 2] = [FileCategory::Document, FileCategory::Code];

#[derive(Default)]
pub struct TokenCosineAlgorithm {
    preprocessor: TokenPreprocessor,
    retriever: TokenChunkRetriever,
    comparator: TokenCoverageComparator,
}

impl DetectionAlgorithm for TokenCosineAlgorithm {
    fn descriptor(&self) -> AlgorithmDescriptor {
        AlgorithmDescriptor {
            id: "token-cosine",
            display_name: "IDF 短词组覆盖率",
            description: "自动忽略常见的公共内容，查找文件之间连续重复的短片段，并以单个文件为单位给出相似度。",
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
struct TokenPreprocessor;

impl Preprocessor for TokenPreprocessor {
    fn preprocess(&self, file: &ParsedFile) -> Result<PreparedFile, DetectionError> {
        prepare(file, TOKEN_KIND, token_features(file))
    }
}

#[derive(Default)]
struct TokenChunkRetriever;

impl Retriever for TokenChunkRetriever {
    fn build_index(
        &self,
        corpus: &[PreparedFile],
    ) -> Result<Box<dyn RetrievalIndex>, DetectionError> {
        build_chunk_index(corpus, TOKEN_KIND)
    }
}

#[derive(Default)]
struct TokenCoverageComparator;

impl Comparator for TokenCoverageComparator {
    fn compare(
        &self,
        query: &PreparedFile,
        source: &PreparedFile,
        weights: &dyn FeatureWeightProvider,
    ) -> Result<ComparisonEvidence, DetectionError> {
        compare_feature_sequences(query, source, TOKEN_KIND, 3, weights)
    }
}

#[cfg(test)]
mod tests {
    use crate::{detection::DetectionPipeline, domain::FileId, parser::ParsedFile};

    use super::*;

    #[test]
    fn retrieves_and_compares_similar_text() {
        let algorithm = TokenCosineAlgorithm::default();
        let pipeline = DetectionPipeline::new(&algorithm);
        let first = parsed(1, "rust ownership borrowing lifetime safety rules");
        let second = parsed(
            2,
            "rust ownership borrowing lifetime safety rules and examples",
        );
        let unrelated = parsed(3, "database transaction isolation index query planner");
        let prepared: Vec<_> = [&first, &second, &unrelated]
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
        let similar = pipeline
            .compare(&prepared[0], &prepared[1])
            .unwrap()
            .similarity;
        let different = pipeline
            .compare(&prepared[0], &prepared[2])
            .unwrap()
            .similarity;
        assert!(similar > 0.95);
        assert_eq!(different, 0.0);
    }

    fn parsed(id: u64, text: &str) -> ParsedFile {
        ParsedFile {
            file_id: FileId(id),
            category: FileCategory::Document,
            text: text.into(),
            locations: Vec::new(),
        }
    }
}
