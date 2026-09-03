use std::{
    collections::{HashMap, HashSet},
    sync::OnceLock,
};

use jieba_rs::{Jieba, TokenizeMode};
use serde::{Deserialize, Serialize};

use crate::{
    detection::{
        Candidate, ComparisonEvidence, DetectionError, FeatureWeightProvider, PreparedFile,
        RetrievalIndex,
    },
    domain::{AnalysisUnitRange, FileId, RiskRegion, TextRange},
    parser::ParsedFile,
};

pub const SHINGLE_KIND: u8 = 1;
pub const TOKEN_KIND: u8 = 2;
pub const WINNOWING_KIND: u8 = 3;

const DOCUMENT_CHUNK_TOKENS: usize = 96;
const CODE_CHUNK_TOKENS: usize = 80;
const MAX_HASH_OCCURRENCES: usize = 128;
const MAX_RISK_REGIONS: usize = 200;
const MIN_CHUNK_RELEVANCE: f32 = 0.30;
const MIN_INFORMATIVE_CHAIN_UNITS: u64 = 8;
const MIN_UNIQUE_FEATURE_EQUIVALENTS: f32 = 3.0;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Feature {
    pub hash: u64,
    pub start: u64,
    pub end: u64,
    pub unit_start: u64,
    pub unit_end: u64,
    pub chunk: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeaturePayload {
    pub kind: u8,
    pub features: Vec<Feature>,
}

#[derive(Debug, Clone)]
pub struct FeatureSequence {
    features: Vec<Feature>,
    analysis_unit_count: u64,
}

#[derive(Debug, Clone)]
struct Token {
    hash: u64,
    start: u64,
    end: u64,
    chunk: u32,
}

#[derive(Debug, Clone, Copy)]
struct MatchChain {
    query_start: usize,
    query_end: usize,
    source_start: usize,
    source_end: usize,
    length: usize,
    evidence_weight: f32,
}

pub fn prepare(
    file: &ParsedFile,
    kind: u8,
    sequence: FeatureSequence,
) -> Result<PreparedFile, DetectionError> {
    let payload = serde_json::to_vec(&FeaturePayload {
        kind,
        features: sequence.features,
    })
    .map_err(|error| DetectionError::new(format!("特征序列化失败：{error}")))?;
    Ok(PreparedFile {
        file_id: file.file_id,
        format_version: 3,
        analysis_unit_count: sequence.analysis_unit_count,
        payload,
    })
}

pub fn decode(file: &PreparedFile, kind: u8) -> Result<FeaturePayload, DetectionError> {
    if file.format_version != 3 {
        return Err(DetectionError::new("不支持的预处理特征版本"));
    }
    let payload: FeaturePayload = serde_json::from_slice(&file.payload)
        .map_err(|error| DetectionError::new(format!("特征反序列化失败：{error}")))?;
    if payload.kind != kind {
        return Err(DetectionError::new("算法与预处理特征类型不匹配"));
    }
    Ok(payload)
}

pub fn shingle_features(file: &ParsedFile, width: usize) -> FeatureSequence {
    feature_sequence(file, |tokens| ngram_features(tokens, width))
}

pub fn token_features(file: &ParsedFile) -> FeatureSequence {
    feature_sequence(file, |tokens| ngram_features(tokens, 3))
}

pub fn winnowing_features(
    file: &ParsedFile,
    gram_width: usize,
    window_width: usize,
) -> FeatureSequence {
    let tokens = tokens_for(file);
    let grams = ngram_features(&tokens, gram_width);
    let mut selected = Vec::new();
    for group in feature_chunks(&grams) {
        if group.len() <= window_width.saturating_mul(2) || window_width == 0 {
            selected.extend_from_slice(group);
            continue;
        }

        let mut last_index = None;
        for start in 0..=group.len() - window_width {
            let (relative, _) = group[start..start + window_width]
                .iter()
                .enumerate()
                .rev()
                .min_by_key(|(_, feature)| feature.hash)
                .expect("指纹窗口不能为空");
            let index = start + relative;
            if last_index != Some(index) {
                selected.push(group[index]);
                last_index = Some(index);
            }
        }
    }
    FeatureSequence {
        features: selected,
        analysis_unit_count: tokens.len() as u64,
    }
}

fn feature_sequence(
    file: &ParsedFile,
    build: impl FnOnce(&[Token]) -> Vec<Feature>,
) -> FeatureSequence {
    let tokens = tokens_for(file);
    FeatureSequence {
        features: build(&tokens),
        analysis_unit_count: tokens.len() as u64,
    }
}

pub fn build_chunk_index(
    corpus: &[PreparedFile],
    kind: u8,
) -> Result<Box<dyn RetrievalIndex>, DetectionError> {
    let mut postings: HashMap<u64, Vec<(FileId, u32)>> = HashMap::new();
    let mut document_frequencies: HashMap<u64, usize> = HashMap::new();
    let mut exact_signatures: HashMap<u64, Vec<FileId>> = HashMap::new();

    for file in corpus {
        let payload = decode(file, kind)?;
        exact_signatures
            .entry(feature_sequence_hash(&payload.features))
            .or_default()
            .push(file.file_id);
        for hash in payload
            .features
            .iter()
            .map(|feature| feature.hash)
            .collect::<HashSet<_>>()
        {
            *document_frequencies.entry(hash).or_default() += 1;
        }
        for chunk in feature_chunks(&payload.features) {
            let unique: HashSet<_> = chunk.iter().map(|feature| feature.hash).collect();
            for hash in unique {
                postings
                    .entry(hash)
                    .or_default()
                    .push((file.file_id, chunk[0].chunk));
            }
        }
    }

    Ok(Box::new(ChunkIndex {
        kind,
        postings,
        document_frequencies,
        exact_signatures,
        document_count: corpus.len(),
    }))
}

pub fn compare_feature_sequences(
    query: &PreparedFile,
    source: &PreparedFile,
    kind: u8,
    minimum_chain: usize,
    weights: &dyn FeatureWeightProvider,
) -> Result<ComparisonEvidence, DetectionError> {
    let query_payload = decode(query, kind)?;
    let source_payload = decode(source, kind)?;
    let query_features = query_payload.features;
    let source_features = source_payload.features;
    if query_features.is_empty() || source_features.is_empty() || query.analysis_unit_count == 0 {
        return Ok(empty_evidence(query.analysis_unit_count));
    }
    let feature_weights: Vec<_> = query_features
        .iter()
        .map(|feature| sanitize_weight(weights.feature_weight(feature.hash)))
        .collect();
    let total_feature_weight: f32 = feature_weights.iter().sum();
    if query.analysis_unit_count == source.analysis_unit_count
        && query_features.len() == source_features.len()
        && query_features
            .iter()
            .zip(&source_features)
            .all(|(left, right)| left.hash == right.hash)
    {
        return Ok(ComparisonEvidence {
            similarity: 1.0,
            weighted_similarity: 1.0,
            query_unit_count: query.analysis_unit_count,
            matched_unit_count: query.analysis_unit_count,
            matched_unit_ranges: vec![AnalysisUnitRange {
                start: 0,
                end: query.analysis_unit_count,
            }],
            risk_regions: vec![RiskRegion {
                query_range: TextRange {
                    start: query_features[0].start,
                    end: query_features[query_features.len() - 1].end,
                },
                source_range: Some(TextRange {
                    start: source_features[0].start,
                    end: source_features[source_features.len() - 1].end,
                }),
                score: 1.0,
            }],
        });
    }

    let mut positions: HashMap<u64, Vec<usize>> = HashMap::new();
    for (index, feature) in source_features.iter().enumerate() {
        positions.entry(feature.hash).or_default().push(index);
    }

    let required = minimum_chain
        .min(query_features.len())
        .min(source_features.len())
        .max(1);
    let evidence_floor = sanitize_weight(weights.evidence_floor());
    let mut weight_prefix = Vec::with_capacity(feature_weights.len() + 1);
    weight_prefix.push(0.0);
    for weight in &feature_weights {
        weight_prefix.push(weight_prefix.last().copied().unwrap_or_default() + weight);
    }
    let mut previous: HashMap<usize, usize> = HashMap::new();
    let mut chains = Vec::new();

    for (query_index, feature) in query_features.iter().enumerate() {
        let mut current = HashMap::new();
        if let Some(source_indexes) = positions.get(&feature.hash)
            && source_indexes.len() <= MAX_HASH_OCCURRENCES
        {
            for &source_index in source_indexes {
                let length = source_index
                    .checked_sub(1)
                    .and_then(|prior| previous.get(&prior))
                    .copied()
                    .unwrap_or_default()
                    + 1;
                current.insert(source_index, length);
                if length >= required {
                    let query_start = query_index + 1 - length;
                    let evidence_weight =
                        weight_prefix[query_index + 1] - weight_prefix[query_start];
                    let matched_units = query_features[query_index]
                        .unit_end
                        .saturating_sub(query_features[query_start].unit_start);
                    if evidence_weight + f32::EPSILON < evidence_floor
                        || (evidence_floor > f32::EPSILON
                            && matched_units < MIN_INFORMATIVE_CHAIN_UNITS)
                    {
                        continue;
                    }
                    chains.push(MatchChain {
                        query_start,
                        query_end: query_index,
                        source_start: source_index + 1 - length,
                        source_end: source_index,
                        length,
                        evidence_weight,
                    });
                }
            }
        }
        previous = current;
    }

    chains.sort_by(|left, right| {
        right
            .evidence_weight
            .total_cmp(&left.evidence_weight)
            .then_with(|| right.length.cmp(&left.length))
            .then_with(|| left.query_start.cmp(&right.query_start))
            .then_with(|| left.source_start.cmp(&right.source_start))
    });

    let mut covered = vec![false; query_features.len()];
    let mut selected = Vec::new();
    for chain in chains {
        if covered[chain.query_start..=chain.query_end]
            .iter()
            .any(|covered| *covered)
        {
            continue;
        }
        covered[chain.query_start..=chain.query_end].fill(true);
        selected.push(chain);
    }

    selected.sort_by_key(|chain| chain.query_start);
    let matched_unit_ranges = merge_unit_ranges(selected.iter().map(|chain| AnalysisUnitRange {
        start: query_features[chain.query_start].unit_start,
        end: query_features[chain.query_end].unit_end,
    }));
    let matched_unit_count = matched_unit_ranges
        .iter()
        .map(|range| range.end.saturating_sub(range.start))
        .sum();
    let similarity = matched_unit_count as f32 / query.analysis_unit_count as f32;
    let matched_feature_weight: f32 = selected.iter().map(|chain| chain.evidence_weight).sum();
    let weighted_similarity = if total_feature_weight <= f32::EPSILON {
        0.0
    } else {
        (matched_feature_weight / total_feature_weight).clamp(0.0, 1.0)
    };
    let risk_regions = selected
        .iter()
        .take(MAX_RISK_REGIONS)
        .map(|chain| RiskRegion {
            query_range: TextRange {
                start: query_features[chain.query_start].start,
                end: query_features[chain.query_end].end,
            },
            source_range: Some(TextRange {
                start: source_features[chain.source_start].start,
                end: source_features[chain.source_end].end,
            }),
            score: similarity,
        })
        .collect();
    Ok(ComparisonEvidence {
        similarity,
        weighted_similarity,
        query_unit_count: query.analysis_unit_count,
        matched_unit_count,
        matched_unit_ranges,
        risk_regions,
    })
}

fn empty_evidence(query_unit_count: u64) -> ComparisonEvidence {
    ComparisonEvidence {
        similarity: 0.0,
        weighted_similarity: 0.0,
        query_unit_count,
        matched_unit_count: 0,
        matched_unit_ranges: Vec::new(),
        risk_regions: Vec::new(),
    }
}

fn sanitize_weight(weight: f32) -> f32 {
    if weight.is_finite() && weight > 0.0 {
        weight
    } else {
        0.0
    }
}

pub fn merge_unit_ranges(
    ranges: impl IntoIterator<Item = AnalysisUnitRange>,
) -> Vec<AnalysisUnitRange> {
    let mut ranges: Vec<_> = ranges
        .into_iter()
        .filter(|range| range.end > range.start)
        .collect();
    ranges.sort_by_key(|range| (range.start, range.end));
    let mut merged: Vec<AnalysisUnitRange> = Vec::new();
    for range in ranges {
        if let Some(previous) = merged.last_mut()
            && range.start <= previous.end
        {
            previous.end = previous.end.max(range.end);
        } else {
            merged.push(range);
        }
    }
    merged
}

struct ChunkIndex {
    kind: u8,
    postings: HashMap<u64, Vec<(FileId, u32)>>,
    document_frequencies: HashMap<u64, usize>,
    exact_signatures: HashMap<u64, Vec<FileId>>,
    document_count: usize,
}

impl FeatureWeightProvider for ChunkIndex {
    fn feature_weight(&self, hash: u64) -> f32 {
        inverse_frequency(
            self.document_count,
            self.document_frequencies.get(&hash).copied().unwrap_or(0),
        )
    }

    fn evidence_floor(&self) -> f32 {
        inverse_frequency(self.document_count, 0) * MIN_UNIQUE_FEATURE_EQUIVALENTS
    }
}

impl RetrievalIndex for ChunkIndex {
    fn retrieve(
        &self,
        query: &PreparedFile,
        limit: usize,
    ) -> Result<Vec<Candidate>, DetectionError> {
        let payload = decode(query, self.kind)?;
        let mut file_scores: HashMap<FileId, f32> = HashMap::new();
        if let Some(file_ids) = self
            .exact_signatures
            .get(&feature_sequence_hash(&payload.features))
        {
            for &file_id in file_ids {
                file_scores.insert(file_id, 1.0);
            }
        }

        for query_chunk in feature_chunks(&payload.features) {
            let query_hashes: HashSet<_> = query_chunk.iter().map(|feature| feature.hash).collect();
            let usable: Vec<_> = query_hashes
                .into_iter()
                .filter_map(|hash| {
                    self.postings.get(&hash)?;
                    let weight = self.feature_weight(hash);
                    (weight > f32::EPSILON).then_some((hash, weight))
                })
                .collect();
            if usable.is_empty() {
                continue;
            }

            let denominator: f32 = usable.iter().map(|(_, weight)| *weight).sum();
            let mut chunk_scores: HashMap<(FileId, u32), (f32, usize)> = HashMap::new();
            for (hash, weight) in usable {
                if let Some(postings) = self.postings.get(&hash) {
                    for &key in postings {
                        let entry = chunk_scores.entry(key).or_default();
                        entry.0 += weight;
                        entry.1 += 1;
                    }
                }
            }

            for ((file_id, _), (score, matched)) in chunk_scores {
                let minimum_hits = 2.min(query_chunk.len());
                if matched < minimum_hits || denominator <= f32::EPSILON {
                    continue;
                }
                let relevance = (score / denominator).clamp(0.0, 1.0);
                if relevance < MIN_CHUNK_RELEVANCE {
                    continue;
                }
                file_scores
                    .entry(file_id)
                    .and_modify(|current| *current = current.max(relevance))
                    .or_insert(relevance);
            }
        }

        let mut candidates: Vec<_> = file_scores
            .into_iter()
            .map(|(file_id, relevance)| Candidate { file_id, relevance })
            .collect();
        candidates.sort_by(|left, right| {
            right
                .relevance
                .total_cmp(&left.relevance)
                .then_with(|| left.file_id.cmp(&right.file_id))
        });
        candidates.truncate(limit);
        Ok(candidates)
    }
}

fn tokens_for(file: &ParsedFile) -> Vec<Token> {
    match file.category {
        crate::domain::FileCategory::Document => document_tokens(&file.text),
        crate::domain::FileCategory::Code => code_tokens(&file.text),
    }
}

fn document_tokens(text: &str) -> Vec<Token> {
    let mut result = Vec::new();
    let mut chunk = 0_u32;
    let mut chunk_size = 0_usize;

    for (start, end) in document_segments(text) {
        let segment = &text[start..end];
        let words: Vec<_> = jieba()
            .tokenize(segment, TokenizeMode::Default, true)
            .into_iter()
            .filter(|token| token.word.chars().any(char::is_alphanumeric))
            .collect();
        if words.is_empty() {
            continue;
        }
        if chunk_size > 0 && chunk_size + words.len() > DOCUMENT_CHUNK_TOKENS {
            chunk += 1;
            chunk_size = 0;
        }
        for word in words {
            if chunk_size == DOCUMENT_CHUNK_TOKENS {
                chunk += 1;
                chunk_size = 0;
            }
            result.push(Token {
                hash: hash_normalized(word.word),
                start: (start + word.byte_start) as u64,
                end: (start + word.byte_end) as u64,
                chunk,
            });
            chunk_size += 1;
        }
    }
    result
}

fn document_segments(text: &str) -> Vec<(usize, usize)> {
    let mut segments = Vec::new();
    let mut start = 0_usize;
    for (index, character) in text.char_indices() {
        if matches!(
            character,
            '。' | '！' | '？' | '；' | '.' | '!' | '?' | ';' | '\n'
        ) {
            let end = index + character.len_utf8();
            push_trimmed_segment(text, start, end, &mut segments);
            start = end;
        }
    }
    push_trimmed_segment(text, start, text.len(), &mut segments);
    segments
}

fn push_trimmed_segment(text: &str, start: usize, end: usize, segments: &mut Vec<(usize, usize)>) {
    let slice = &text[start..end];
    let Some((leading, _)) = slice
        .char_indices()
        .find(|(_, character)| !character.is_whitespace())
    else {
        return;
    };
    let trailing = slice
        .char_indices()
        .rev()
        .find(|(_, character)| !character.is_whitespace())
        .map(|(index, character)| index + character.len_utf8())
        .unwrap_or_default();
    segments.push((start + leading, start + trailing));
}

fn code_tokens(text: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut characters = text.char_indices().peekable();
    let mut line_is_empty = true;

    while let Some((start, character)) = characters.next() {
        if character.is_whitespace() {
            if character == '\n' {
                line_is_empty = true;
            }
            continue;
        }

        if character == '/' && characters.peek().is_some_and(|(_, next)| *next == '/') {
            characters.next();
            for (_, next) in characters.by_ref() {
                if next == '\n' {
                    line_is_empty = true;
                    break;
                }
            }
            continue;
        }
        if character == '/' && characters.peek().is_some_and(|(_, next)| *next == '*') {
            characters.next();
            let mut previous = '\0';
            for (_, next) in characters.by_ref() {
                if previous == '*' && next == '/' {
                    break;
                }
                previous = next;
            }
            continue;
        }
        if character == '#'
            && line_is_empty
            && !characters.peek().is_some_and(|(_, next)| *next == '[')
        {
            for (_, next) in characters.by_ref() {
                if next == '\n' {
                    line_is_empty = true;
                    break;
                }
            }
            continue;
        }

        line_is_empty = false;
        let (end, normalized) = if character == '"' || character == '\'' || character == '`' {
            let quote = character;
            let mut end = start + character.len_utf8();
            let mut escaped = false;
            for (index, next) in characters.by_ref() {
                end = index + next.len_utf8();
                if next == quote && !escaped {
                    break;
                }
                escaped = next == '\\' && !escaped;
                if next != '\\' {
                    escaped = false;
                }
            }
            (end, "<字符串>".to_owned())
        } else if character.is_alphabetic() || character == '_' {
            let mut end = start + character.len_utf8();
            while let Some(&(index, next)) = characters.peek() {
                if !(next.is_alphanumeric() || next == '_') {
                    break;
                }
                characters.next();
                end = index + next.len_utf8();
            }
            (end, text[start..end].to_lowercase())
        } else if character.is_numeric() {
            let mut end = start + character.len_utf8();
            while let Some(&(index, next)) = characters.peek() {
                if !(next.is_alphanumeric() || matches!(next, '.' | '_')) {
                    break;
                }
                characters.next();
                end = index + next.len_utf8();
            }
            (end, "<数字>".to_owned())
        } else {
            let mut end = start + character.len_utf8();
            let mut operator = character.to_string();
            if let Some(&(index, next)) = characters.peek() {
                let pair = format!("{character}{next}");
                if matches!(
                    pair.as_str(),
                    "==" | "!="
                        | "<="
                        | ">="
                        | "->"
                        | "=>"
                        | "::"
                        | "&&"
                        | "||"
                        | "+="
                        | "-="
                        | "*="
                        | "/="
                        | "%="
                        | "++"
                        | "--"
                        | "<<"
                        | ">>"
                ) {
                    characters.next();
                    end = index + next.len_utf8();
                    operator = pair;
                }
            }
            (end, operator)
        };

        tokens.push(Token {
            hash: hash_bytes(normalized.as_bytes()),
            start: start as u64,
            end: end as u64,
            chunk: (tokens.len() / CODE_CHUNK_TOKENS) as u32,
        });
    }
    tokens
}

fn ngram_features(tokens: &[Token], width: usize) -> Vec<Feature> {
    let mut features = Vec::new();
    let mut unit_offset = 0_usize;
    for chunk in token_chunks(tokens) {
        if chunk.is_empty() {
            continue;
        }
        let actual_width = width.min(chunk.len());
        for (relative_index, window) in chunk.windows(actual_width).enumerate() {
            let hash = window
                .iter()
                .fold(FNV_OFFSET, |hash, token| mix64(hash ^ token.hash));
            features.push(Feature {
                hash,
                start: window[0].start,
                end: window[window.len() - 1].end,
                unit_start: (unit_offset + relative_index) as u64,
                unit_end: (unit_offset + relative_index + window.len()) as u64,
                chunk: window[0].chunk,
            });
        }
        unit_offset += chunk.len();
    }
    features
}

fn token_chunks(tokens: &[Token]) -> Vec<&[Token]> {
    contiguous_groups(tokens, |token| token.chunk)
}

fn feature_chunks(features: &[Feature]) -> Vec<&[Feature]> {
    contiguous_groups(features, |feature| feature.chunk)
}

fn contiguous_groups<T, K: PartialEq + Copy>(items: &[T], key: impl Fn(&T) -> K) -> Vec<&[T]> {
    let mut groups = Vec::new();
    let mut start = 0;
    while start < items.len() {
        let group_key = key(&items[start]);
        let mut end = start + 1;
        while end < items.len() && key(&items[end]) == group_key {
            end += 1;
        }
        groups.push(&items[start..end]);
        start = end;
    }
    groups
}

fn inverse_frequency(total: usize, frequency: usize) -> f32 {
    if total <= 1 {
        // A one-document corpus has no frequency contrast. Keep the ordinary
        // minimum-chain behavior instead of suppressing every partial match.
        1.0
    } else {
        ((total as f32 + 1.0) / (frequency.min(total) as f32 + 1.0)).ln()
    }
}

fn feature_sequence_hash(features: &[Feature]) -> u64 {
    features
        .iter()
        .fold(FNV_OFFSET, |hash, feature| mix64(hash ^ feature.hash))
}

fn jieba() -> &'static Jieba {
    static INSTANCE: OnceLock<Jieba> = OnceLock::new();
    INSTANCE.get_or_init(Jieba::new)
}

fn hash_normalized(value: &str) -> u64 {
    hash_bytes(value.to_lowercase().as_bytes())
}

const FNV_OFFSET: u64 = 0xcbf29ce484222325;
const FNV_PRIME: u64 = 0x100000001b3;

fn hash_bytes(bytes: &[u8]) -> u64 {
    bytes
        .iter()
        .fold(FNV_OFFSET, |hash, byte| fnv_step(hash, *byte))
}

fn fnv_step(hash: u64, byte: u8) -> u64 {
    (hash ^ u64::from(byte)).wrapping_mul(FNV_PRIME)
}

fn mix64(mut value: u64) -> u64 {
    value ^= value >> 30;
    value = value.wrapping_mul(0xbf58476d1ce4e5b9);
    value ^= value >> 27;
    value = value.wrapping_mul(0x94d049bb133111eb);
    value ^ (value >> 31)
}

#[cfg(test)]
mod tests {
    use crate::{detection::FeatureWeightProvider, domain::FileCategory, parser::ParsedFile};

    use super::*;

    struct UniformTestWeights;

    impl FeatureWeightProvider for UniformTestWeights {
        fn feature_weight(&self, _hash: u64) -> f32 {
            1.0
        }

        fn evidence_floor(&self) -> f32 {
            0.0
        }
    }

    #[test]
    fn document_tokenization_keeps_words_and_utf8_boundaries() {
        let text = "机器学习方法可以分析数据。Rust ownership keeps programs safe.";
        let tokens = document_tokens(text);
        let slices: Vec<_> = tokens
            .iter()
            .map(|token| &text[token.start as usize..token.end as usize])
            .collect();

        assert!(slices.iter().any(|word| word.contains('机')));
        assert!(!slices.contains(&"机器学习方法可以分析数据"));
        assert!(slices.contains(&"Rust"));
        assert!(slices.contains(&"ownership"));
        assert!(!slices.contains(&"ust"));
    }

    #[test]
    fn code_tokenization_keeps_identifiers_and_ignores_comments() {
        let text = "// counter_value is old\nlet counter_value = call(42);";
        let tokens = code_tokens(text);
        let slices: Vec<_> = tokens
            .iter()
            .map(|token| &text[token.start as usize..token.end as usize])
            .collect();

        assert_eq!(
            slices
                .iter()
                .filter(|value| **value == "counter_value")
                .count(),
            1
        );
        assert!(!slices.contains(&"old"));
        assert!(slices.contains(&"42"));
    }

    #[test]
    fn exact_and_contained_sequences_use_query_coverage() {
        let query = parsed(
            1,
            FileCategory::Document,
            "alpha beta gamma delta epsilon zeta eta theta",
        );
        let exact = parsed(2, FileCategory::Document, &query.text);
        let containing = parsed(
            3,
            FileCategory::Document,
            "prefix words alpha beta gamma delta epsilon zeta eta theta suffix words",
        );
        let query_text = query.text.clone();
        let query = prepare(&query, TOKEN_KIND, token_features(&query)).unwrap();
        let exact = prepare(&exact, TOKEN_KIND, token_features(&exact)).unwrap();
        let containing = prepare(&containing, TOKEN_KIND, token_features(&containing)).unwrap();

        let exact_evidence =
            compare_feature_sequences(&query, &exact, TOKEN_KIND, 3, &UniformTestWeights).unwrap();
        assert_eq!(exact_evidence.similarity, 1.0);
        assert_eq!(exact_evidence.matched_unit_count, 8);
        assert_eq!(exact_evidence.query_unit_count, 8);
        assert_eq!(exact_evidence.risk_regions.len(), 1);
        let range = exact_evidence.risk_regions[0].query_range;
        assert_eq!(
            &query_text[range.start as usize..range.end as usize],
            query_text
        );
        assert!(
            compare_feature_sequences(&query, &containing, TOKEN_KIND, 3, &UniformTestWeights,)
                .unwrap()
                .similarity
                > 0.95
        );
    }

    #[test]
    fn corpus_idf_rejects_common_only_chain_and_keeps_rare_chain() {
        let common = "standard common phrase appears here";
        let mut corpus: Vec<_> = (0..10)
            .map(|id| {
                let file = parsed(
                    id,
                    FileCategory::Document,
                    &format!("{common} ordinary filler document number {id}"),
                );
                prepare(&file, TOKEN_KIND, token_features(&file)).unwrap()
            })
            .collect();
        let common_source_file = parsed(
            20,
            FileCategory::Document,
            &format!("{common} unrelated ending material"),
        );
        let rare_source_file = parsed(
            21,
            FileCategory::Document,
            "quantum lattice entropy beacon rotates silently beyond hidden orbital vectors",
        );
        let common_source = prepare(
            &common_source_file,
            TOKEN_KIND,
            token_features(&common_source_file),
        )
        .unwrap();
        let rare_source = prepare(
            &rare_source_file,
            TOKEN_KIND,
            token_features(&rare_source_file),
        )
        .unwrap();
        corpus.extend([common_source.clone(), rare_source.clone()]);
        let index = build_chunk_index(&corpus, TOKEN_KIND).unwrap();

        let common_query_file = parsed(
            30,
            FileCategory::Document,
            &format!("prefix {common} query ending"),
        );
        let rare_query_file = parsed(
            31,
            FileCategory::Document,
            "prefix quantum lattice entropy beacon rotates silently beyond hidden orbital vectors suffix",
        );
        let common_query = prepare(
            &common_query_file,
            TOKEN_KIND,
            token_features(&common_query_file),
        )
        .unwrap();
        let rare_query = prepare(
            &rare_query_file,
            TOKEN_KIND,
            token_features(&rare_query_file),
        )
        .unwrap();

        let common_evidence =
            compare_feature_sequences(&common_query, &common_source, TOKEN_KIND, 3, index.as_ref())
                .unwrap();
        let rare_evidence =
            compare_feature_sequences(&rare_query, &rare_source, TOKEN_KIND, 3, index.as_ref())
                .unwrap();

        assert_eq!(common_evidence.similarity, 0.0);
        assert_eq!(common_evidence.weighted_similarity, 0.0);
        assert!(rare_evidence.similarity > 0.5);
        assert!(rare_evidence.weighted_similarity > 0.0);
    }

    #[test]
    fn merged_unit_ranges_count_overlapping_sources_only_once() {
        assert_eq!(
            merge_unit_ranges([
                AnalysisUnitRange { start: 2, end: 8 },
                AnalysisUnitRange { start: 5, end: 10 },
                AnalysisUnitRange { start: 12, end: 15 },
            ]),
            vec![
                AnalysisUnitRange { start: 2, end: 10 },
                AnalysisUnitRange { start: 12, end: 15 },
            ]
        );
    }

    #[test]
    fn chunk_index_retrieves_local_overlap_from_large_corpus() {
        let query = parsed(
            999,
            FileCategory::Document,
            "distinctive phrase about distributed indexing and retrieval accuracy",
        );
        let mut files: Vec<_> = (0..300)
            .map(|id| {
                parsed(
                    id,
                    FileCategory::Document,
                    &format!("ordinary unrelated document number {id} with filler material"),
                )
            })
            .collect();
        files.push(parsed(
            500,
            FileCategory::Document,
            "intro distinctive phrase about distributed indexing and retrieval accuracy ending",
        ));
        let prepared: Vec<_> = files
            .iter()
            .map(|file| prepare(file, TOKEN_KIND, token_features(file)).unwrap())
            .collect();
        let query = prepare(&query, TOKEN_KIND, token_features(&query)).unwrap();
        let candidates = build_chunk_index(&prepared, TOKEN_KIND)
            .unwrap()
            .retrieve(&query, 10)
            .unwrap();

        assert!(candidates.iter().any(|item| item.file_id == FileId(500)));
        assert!(candidates.len() < prepared.len());
    }

    fn parsed(id: u64, category: FileCategory, text: &str) -> ParsedFile {
        ParsedFile {
            file_id: FileId(id),
            category,
            text: text.into(),
            locations: Vec::new(),
        }
    }
}
