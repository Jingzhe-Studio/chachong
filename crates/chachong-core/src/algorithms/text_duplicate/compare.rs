use crate::{
    algorithms::features::{SHINGLE_KIND, compare_feature_sequences},
    detection::{
        Comparator, ComparisonEvidence, DetectionError, FeatureWeightProvider, PreparedFile,
    },
};

#[derive(Default)]
pub(super) struct TextDuplicateComparator;

impl Comparator for TextDuplicateComparator {
    fn compare(
        &self,
        query: &PreparedFile,
        source: &PreparedFile,
        weights: &dyn FeatureWeightProvider,
    ) -> Result<ComparisonEvidence, DetectionError> {
        compare_feature_sequences(query, source, SHINGLE_KIND, 2, weights)
    }
}
