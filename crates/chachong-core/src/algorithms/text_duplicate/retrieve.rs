use crate::{
    algorithms::features::{SHINGLE_KIND, build_chunk_index},
    detection::{DetectionError, PreparedFile, RetrievalIndex, Retriever},
};

#[derive(Default)]
pub(super) struct TextDuplicateRetriever;

impl Retriever for TextDuplicateRetriever {
    fn build_index(
        &self,
        corpus: &[PreparedFile],
    ) -> Result<Box<dyn RetrievalIndex>, DetectionError> {
        build_chunk_index(corpus, SHINGLE_KIND)
    }
}
