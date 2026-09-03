use std::{
    collections::{HashMap, HashSet},
    sync::OnceLock,
};

use jieba_rs::{Jieba, TokenizeMode};
use serde::{Deserialize, Serialize};

use crate::{
    detection::{Candidate, DetectionError, PreparedFile, RetrievalIndex},
    domain::{FileId, RiskRegion, TextRange},
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

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Feature {
    pub hash: u64,
    pub start: u64,
    pub end: u64,
    pub chunk: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeaturePayload {
    pub kind: u8,
    pub features: Vec<Feature>,
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
}

pub fn prepare(
    file: &ParsedFile,
    kind: u8,
    features: Vec<Feature>,
) -> Result<PreparedFile, DetectionError> {
    let payload = serde_json::to_vec(&FeaturePayload { kind, features })
        .map_err(|error| DetectionError::new(format!("特征序列化失败：{error}")))?;
    Ok(PreparedFile {
        file_id: file.file_id,
        format_version: 2,
        payload,
    })
}

pub fn decode(file: &PreparedFile, kind: u8) -> Result<FeaturePayload, DetectionError> {
    if file.format_version != 2 {
        return Err(DetectionError::new("不支持的预处理特征版本"));
    }
    let payload: FeaturePayload = serde_json::from_slice(&file.payload)
        .map_err(|error| DetectionError::new(format!("特征反序列化失败：{error}")))?;
    if payload.kind != kind {
        return Err(DetectionError::new("算法与预处理特征类型不匹配"));
    }
    Ok(payload)
}

pub fn shingle_features(file: &ParsedFile, width: usize) -> Vec<Feature> {
    ngram_features(&tokens_for(file), width)
}

pub fn token_features(file: &ParsedFile) -> Vec<Feature> {
    ngram_features(&tokens_for(file), 3)
}

pub fn winnowing_features(
    file: &ParsedFile,
    gram_width: usize,
    window_width: usize,
) -> Vec<Feature> {
    let grams = ngram_features(&tokens_for(file), gram_width);
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
    selected
}

pub fn build_chunk_index(
    corpus: &[PreparedFile],
    kind: u8,
) -> Result<Box<dyn RetrievalIndex>, DetectionError> {
    let mut postings: HashMap<u64, Vec<(FileId, u32)>> = HashMap::new();
    let mut exact_signatures: HashMap<u64, Vec<FileId>> = HashMap::new();
    let mut chunk_count = 0_usize;

    for file in corpus {
        let payload = decode(file, kind)?;
        exact_signatures
            .entry(feature_sequence_hash(&payload.features))
            .or_default()
            .push(file.file_id);
        for chunk in feature_chunks(&payload.features) {
            chunk_count += 1;
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
        exact_signatures,
        chunk_count,
    }))
}

pub fn compare_feature_sequences(
    query: &[Feature],
    source: &[Feature],
    minimum_chain: usize,
) -> (f32, Vec<RiskRegion>) {
    if query.is_empty() || source.is_empty() {
        return (0.0, Vec::new());
    }
    if query.len() == source.len()
        && query
            .iter()
            .zip(source)
            .all(|(left, right)| left.hash == right.hash)
    {
        return (
            1.0,
            vec![RiskRegion {
                query_range: TextRange {
                    start: query[0].start,
                    end: query[query.len() - 1].end,
                },
                source_range: Some(TextRange {
                    start: source[0].start,
                    end: source[source.len() - 1].end,
                }),
                score: 1.0,
            }],
        );
    }

    let mut positions: HashMap<u64, Vec<usize>> = HashMap::new();
    for (index, feature) in source.iter().enumerate() {
        positions.entry(feature.hash).or_default().push(index);
    }

    let required = minimum_chain.min(query.len()).min(source.len()).max(1);
    let mut previous: HashMap<usize, usize> = HashMap::new();
    let mut chains = Vec::new();

    for (query_index, feature) in query.iter().enumerate() {
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
                    chains.push(MatchChain {
                        query_start: query_index + 1 - length,
                        query_end: query_index,
                        source_start: source_index + 1 - length,
                        source_end: source_index,
                        length,
                    });
                }
            }
        }
        previous = current;
    }

    chains.sort_by(|left, right| {
        right
            .length
            .cmp(&left.length)
            .then_with(|| left.query_start.cmp(&right.query_start))
            .then_with(|| left.source_start.cmp(&right.source_start))
    });

    let mut covered = vec![false; query.len()];
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
        if selected.len() == MAX_RISK_REGIONS {
            break;
        }
    }

    let matched = covered.iter().filter(|covered| **covered).count();
    let similarity = matched as f32 / query.len() as f32;
    selected.sort_by_key(|chain| chain.query_start);
    let regions = selected
        .into_iter()
        .map(|chain| RiskRegion {
            query_range: TextRange {
                start: query[chain.query_start].start,
                end: query[chain.query_end].end,
            },
            source_range: Some(TextRange {
                start: source[chain.source_start].start,
                end: source[chain.source_end].end,
            }),
            score: similarity,
        })
        .collect();
    (similarity, regions)
}

struct ChunkIndex {
    kind: u8,
    postings: HashMap<u64, Vec<(FileId, u32)>>,
    exact_signatures: HashMap<u64, Vec<FileId>>,
    chunk_count: usize,
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
                    let frequency = self.postings.get(&hash)?.len();
                    if self.chunk_count >= 8 && frequency * 5 >= self.chunk_count * 4 {
                        return None;
                    }
                    Some((hash, inverse_frequency(self.chunk_count, frequency)))
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
    for chunk in token_chunks(tokens) {
        if chunk.is_empty() {
            continue;
        }
        let actual_width = width.min(chunk.len());
        for window in chunk.windows(actual_width) {
            let hash = window
                .iter()
                .fold(FNV_OFFSET, |hash, token| mix64(hash ^ token.hash));
            features.push(Feature {
                hash,
                start: window[0].start,
                end: window[window.len() - 1].end,
                chunk: window[0].chunk,
            });
        }
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
    ((total as f32 + 1.0) / (frequency as f32 + 0.5)).ln() + 1.0
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
    use crate::{domain::FileCategory, parser::ParsedFile};

    use super::*;

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
        let query = token_features(&query);
        let exact = token_features(&exact);
        let containing = token_features(&containing);

        let (exact_score, exact_regions) = compare_feature_sequences(&query, &exact, 3);
        assert_eq!(exact_score, 1.0);
        assert_eq!(exact_regions.len(), 1);
        let range = exact_regions[0].query_range;
        assert_eq!(
            &query_text[range.start as usize..range.end as usize],
            query_text
        );
        assert!(compare_feature_sequences(&query, &containing, 3).0 > 0.95);
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
