use crate::parser::ParsedFile;

use super::{
    ComparisonEvidence, DetectionAlgorithm, DetectionError, FeatureWeightProvider, PreparedFile,
    RetrievalIndex,
};

struct UniformFeatureWeights;

impl FeatureWeightProvider for UniformFeatureWeights {
    fn feature_weight(&self, _hash: u64) -> f32 {
        1.0
    }

    fn evidence_floor(&self) -> f32 {
        0.0
    }
}

pub struct DetectionPipeline<'algorithm> {
    algorithm: &'algorithm dyn DetectionAlgorithm,
}

impl<'algorithm> DetectionPipeline<'algorithm> {
    pub fn new(algorithm: &'algorithm dyn DetectionAlgorithm) -> Self {
        Self { algorithm }
    }

    pub fn preprocess(&self, file: &ParsedFile) -> Result<PreparedFile, DetectionError> {
        self.algorithm.preprocessor().preprocess(file)
    }

    pub fn build_index(
        &self,
        corpus: &[PreparedFile],
    ) -> Result<Box<dyn RetrievalIndex>, DetectionError> {
        self.algorithm.retriever().build_index(corpus)
    }

    pub fn compare(
        &self,
        query: &PreparedFile,
        source: &PreparedFile,
    ) -> Result<ComparisonEvidence, DetectionError> {
        self.algorithm
            .comparator()
            .compare(query, source, &UniformFeatureWeights)
    }

    pub fn compare_with_weights(
        &self,
        query: &PreparedFile,
        source: &PreparedFile,
        weights: &dyn FeatureWeightProvider,
    ) -> Result<ComparisonEvidence, DetectionError> {
        self.algorithm.comparator().compare(query, source, weights)
    }
}
