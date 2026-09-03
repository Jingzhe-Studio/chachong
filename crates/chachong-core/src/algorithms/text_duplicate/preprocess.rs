use crate::{
    algorithms::features::{SHINGLE_KIND, prepare, shingle_features},
    detection::{DetectionError, PreparedFile, Preprocessor},
    parser::ParsedFile,
};

#[derive(Default)]
pub(super) struct TextDuplicatePreprocessor;

impl Preprocessor for TextDuplicatePreprocessor {
    fn preprocess(&self, file: &ParsedFile) -> Result<PreparedFile, DetectionError> {
        prepare(file, SHINGLE_KIND, shingle_features(file, 5))
    }
}
