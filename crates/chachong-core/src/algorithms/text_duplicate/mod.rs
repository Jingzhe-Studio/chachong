mod compare;
mod preprocess;
mod retrieve;

use crate::{
    detection::{AlgorithmDescriptor, Comparator, DetectionAlgorithm, Preprocessor, Retriever},
    domain::FileCategory,
};

use compare::TextDuplicateComparator;
use preprocess::TextDuplicatePreprocessor;
use retrieve::TextDuplicateRetriever;

static SUPPORTED_CATEGORIES: [FileCategory; 2] = [FileCategory::Document, FileCategory::Code];

#[derive(Default)]
pub struct TextDuplicateAlgorithm {
    preprocessor: TextDuplicatePreprocessor,
    retriever: TextDuplicateRetriever,
    comparator: TextDuplicateComparator,
}

impl DetectionAlgorithm for TextDuplicateAlgorithm {
    fn descriptor(&self) -> AlgorithmDescriptor {
        AlgorithmDescriptor {
            id: "shingle-minhash",
            display_name: "长词组覆盖率",
            description: "按段落和代码块召回候选，再计算当前文件中连续长词组的覆盖比例。",
            supported_categories: &SUPPORTED_CATEGORIES,
        }
    }

    fn preprocessor(&self) -> &dyn Preprocessor {
        &self.preprocessor
    }

    fn retriever(&self) -> &dyn Retriever {
        &self.retriever
    }

    fn comparator(&self) -> &dyn Comparator {
        &self.comparator
    }
}

#[cfg(test)]
mod tests {
    use crate::{detection::DetectionPipeline, domain::FileId, parser::ParsedFile};

    use super::*;

    #[test]
    fn exposes_the_minhash_descriptor() {
        let algorithm = TextDuplicateAlgorithm::default();
        assert_eq!(algorithm.descriptor().id, "shingle-minhash");
    }

    #[test]
    fn minhash_pipeline_retrieves_and_marks_shared_text() {
        let algorithm = TextDuplicateAlgorithm::default();
        let pipeline = DetectionPipeline::new(&algorithm);
        let parsed = |id, text: &str| ParsedFile {
            file_id: FileId(id),
            category: FileCategory::Document,
            text: text.into(),
            locations: Vec::new(),
        };
        let first = pipeline
            .preprocess(&parsed(1, "这是一个包含公共实验步骤和结论的报告"))
            .unwrap();
        let second = pipeline
            .preprocess(&parsed(2, "前言：这是一个包含公共实验步骤和结论的报告。"))
            .unwrap();
        let unrelated = pipeline
            .preprocess(&parsed(3, "数据库使用事务保证并发一致性"))
            .unwrap();
        let index = pipeline
            .build_index(&[first.clone(), second.clone(), unrelated.clone()])
            .unwrap();
        assert!(
            index
                .retrieve(&first, 3)
                .unwrap()
                .iter()
                .any(|candidate| candidate.file_id == FileId(2))
        );
        let evidence = pipeline.compare(&first, &second).unwrap();
        assert!(evidence.similarity > 0.5);
        assert!(!evidence.risk_regions.is_empty());
    }
}
