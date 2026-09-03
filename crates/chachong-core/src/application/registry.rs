use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use crate::{
    algorithms::{
        text_duplicate::TextDuplicateAlgorithm, token_cosine::TokenCosineAlgorithm,
        winnowing::WinnowingAlgorithm,
    },
    detection::{AlgorithmDescriptor, DetectionAlgorithm},
};

use super::{ApplicationError, BatchService, ReferenceLibraryService};

#[derive(Default)]
pub struct AlgorithmRegistry {
    algorithms: BTreeMap<&'static str, Arc<dyn DetectionAlgorithm>>,
}

impl AlgorithmRegistry {
    pub fn register(&mut self, algorithm: Arc<dyn DetectionAlgorithm>) {
        let id = algorithm.descriptor().id;
        self.algorithms.insert(id, algorithm);
    }

    pub fn get(&self, id: &str) -> Option<&dyn DetectionAlgorithm> {
        self.algorithms.get(id).map(Arc::as_ref)
    }

    pub fn resolve(&self, id: &str) -> Option<Arc<dyn DetectionAlgorithm>> {
        self.algorithms.get(id).cloned()
    }

    pub fn descriptors(&self) -> Vec<AlgorithmDescriptor> {
        self.algorithms
            .values()
            .map(|algorithm| algorithm.descriptor())
            .collect()
    }
}

pub struct AppCore {
    algorithms: AlgorithmRegistry,
    references: ReferenceLibraryService,
    batches: BatchService,
}

impl AppCore {
    pub fn open(data_root: impl AsRef<Path>) -> Result<Self, ApplicationError> {
        let mut algorithms = AlgorithmRegistry::default();
        algorithms.register(Arc::new(TextDuplicateAlgorithm::default()));
        algorithms.register(Arc::new(TokenCosineAlgorithm::default()));
        algorithms.register(Arc::new(WinnowingAlgorithm::default()));
        let references = ReferenceLibraryService::open(data_root.as_ref())?;
        let batches = BatchService::open(data_root)?;
        Ok(Self {
            algorithms,
            references,
            batches,
        })
    }

    pub fn algorithms(&self) -> &AlgorithmRegistry {
        &self.algorithms
    }

    pub fn references(&self) -> &ReferenceLibraryService {
        &self.references
    }

    pub fn batches(&self) -> &BatchService {
        &self.batches
    }
}
