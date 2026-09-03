use crate::parser::ParsedFile;

use super::{AlgorithmDescriptor, Candidate, ComparisonEvidence, DetectionError, PreparedFile};

pub trait Preprocessor: Send + Sync {
    fn preprocess(&self, file: &ParsedFile) -> Result<PreparedFile, DetectionError>;
}

pub trait RetrievalIndex: Send + Sync {
    fn retrieve(
        &self,
        query: &PreparedFile,
        limit: usize,
    ) -> Result<Vec<Candidate>, DetectionError>;
}

pub trait Retriever: Send + Sync {
    fn build_index(
        &self,
        corpus: &[PreparedFile],
    ) -> Result<Box<dyn RetrievalIndex>, DetectionError>;
}

pub trait Comparator: Send + Sync {
    fn compare(
        &self,
        query: &PreparedFile,
        source: &PreparedFile,
    ) -> Result<ComparisonEvidence, DetectionError>;
}

/// A statically linked algorithm assembled from three independently testable phases.
pub trait DetectionAlgorithm: Send + Sync {
    fn descriptor(&self) -> AlgorithmDescriptor;
    fn preprocessor(&self) -> &dyn Preprocessor;
    fn retriever(&self) -> &dyn Retriever;
    fn comparator(&self) -> &dyn Comparator;
}
