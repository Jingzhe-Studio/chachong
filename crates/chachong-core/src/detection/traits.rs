use crate::parser::ParsedFile;

use super::{AlgorithmDescriptor, Candidate, ComparisonEvidence, DetectionError, PreparedFile};

pub trait Preprocessor: Send + Sync {
    fn preprocess(&self, file: &ParsedFile) -> Result<PreparedFile, DetectionError>;
}

/// Supplies background-corpus statistics to the exact comparison phase.
///
/// A feature's weight is its inverse document frequency in the available corpus. The
/// evidence floor is expressed in the same units, so common-only chains can be
/// discarded before they become matched Token ranges.
pub trait FeatureWeightProvider: Send + Sync {
    fn feature_weight(&self, hash: u64) -> f32;
    fn evidence_floor(&self) -> f32;
}

pub trait RetrievalIndex: FeatureWeightProvider {
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
        weights: &dyn FeatureWeightProvider,
    ) -> Result<ComparisonEvidence, DetectionError>;
}

/// A statically linked algorithm assembled from three independently testable phases.
pub trait DetectionAlgorithm: Send + Sync {
    fn descriptor(&self) -> AlgorithmDescriptor;
    fn preprocessor(&self) -> &dyn Preprocessor;
    fn retriever(&self) -> &dyn Retriever;
    fn comparator(&self) -> &dyn Comparator;
}
