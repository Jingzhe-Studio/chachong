use crate::{
    algorithms::features::{SHINGLE_KIND, compare_feature_sequences, decode},
    detection::{Comparator, ComparisonEvidence, DetectionError, PreparedFile},
};

#[derive(Default)]
pub(super) struct TextDuplicateComparator;

impl Comparator for TextDuplicateComparator {
    fn compare(
        &self,
        query: &PreparedFile,
        source: &PreparedFile,
    ) -> Result<ComparisonEvidence, DetectionError> {
        let query = decode(query, SHINGLE_KIND)?;
        let source = decode(source, SHINGLE_KIND)?;
        let (similarity, risk_regions) =
            compare_feature_sequences(&query.features, &source.features, 2);
        Ok(ComparisonEvidence {
            similarity,
            risk_regions,
        })
    }
}
