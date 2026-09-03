use crate::parser::ParsedFile;

use super::{ComparisonEvidence, DetectionAlgorithm, DetectionError, PreparedFile, RetrievalIndex};

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
        self.algorithm.comparator().compare(query, source)
    }
}
