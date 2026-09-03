mod pipeline;
mod traits;
mod types;

pub use pipeline::DetectionPipeline;
pub use traits::{
    Comparator, DetectionAlgorithm, FeatureWeightProvider, Preprocessor, RetrievalIndex, Retriever,
};
pub use types::{AlgorithmDescriptor, Candidate, ComparisonEvidence, DetectionError, PreparedFile};
