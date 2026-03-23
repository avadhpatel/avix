use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// ── Vector index storage ───────────────────────────────────────────────────────

/// Stored at `/users/<owner>/memory/<agent>/{episodic,semantic}/index/vectors.idx`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VectorIndex {
    /// Embedding model that produced these vectors.
    /// Used to detect staleness when the model changes.
    pub model: String,
    pub entries: Vec<VectorEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorEntry {
    pub id: String,
    pub vector: Vec<f32>,
    pub updated_at: DateTime<Utc>,
}

/// Returns true if the vector index was built with a different model.
/// A stale index should be rebuilt before use; retrieval falls back to BM25-only.
pub fn is_vector_index_stale(idx: &VectorIndex, current_model: &str) -> bool {
    idx.model.is_empty() || idx.model != current_model
}

// ── BM25 fulltext index storage ───────────────────────────────────────────────

/// Stored at `/users/<owner>/memory/<agent>/{episodic,semantic}/index/fulltext.idx`.
///
/// Pre-computed IDF weights — allow O(1) BM25 scoring without re-scanning all docs.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FulltextIndex {
    pub avg_doc_length: f32,
    /// term → count of documents containing that term
    pub doc_frequencies: HashMap<String, u32>,
    pub doc_count: u32,
    pub updated_at: DateTime<Utc>,
}

// ── Cosine similarity ─────────────────────────────────────────────────────────

/// Compute cosine similarity between two equal-length vectors.
/// Returns 0.0 if either vector has zero magnitude.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    dot / (norm_a * norm_b)
}

/// Find the top-k records in the vector index by cosine similarity to the query vector.
///
/// Returns `(id, similarity)` pairs sorted by similarity descending.
pub fn cosine_search(idx: &VectorIndex, query_vector: &[f32], k: usize) -> Vec<(String, f32)> {
    let mut scored: Vec<(String, f32)> = idx
        .entries
        .iter()
        .map(|e| (e.id.clone(), cosine_similarity(query_vector, &e.vector)))
        .collect();
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(k);
    scored
}

// ── Reciprocal Rank Fusion ────────────────────────────────────────────────────

/// Merge two ranked candidate lists using Reciprocal Rank Fusion (RRF).
///
/// Returns a deduplicated list of record IDs ordered by RRF score (descending).
/// `k` is the RRF smoothing parameter — typically 60.
///
/// RRF score = Σ 1 / (k + rank_i) for each list where the item appears.
pub fn rrf_merge(bm25: Vec<(String, f32)>, vector: Vec<(String, f32)>, k: u32) -> Vec<String> {
    let mut rrf_scores: HashMap<String, f64> = HashMap::new();
    for (rank, (id, _)) in bm25.iter().enumerate() {
        *rrf_scores.entry(id.clone()).or_insert(0.0) += 1.0 / (k as f64 + rank as f64 + 1.0);
    }
    for (rank, (id, _)) in vector.iter().enumerate() {
        *rrf_scores.entry(id.clone()).or_insert(0.0) += 1.0 / (k as f64 + rank as f64 + 1.0);
    }
    let mut ids: Vec<(String, f64)> = rrf_scores.into_iter().collect();
    ids.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    ids.into_iter().map(|(id, _)| id).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cosine_similarity_unit_vectors() {
        let a = vec![1.0f32, 0.0, 0.0];
        let b = vec![1.0f32, 0.0, 0.0];
        assert!((cosine_similarity(&a, &b) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn cosine_similarity_orthogonal() {
        let a = vec![1.0f32, 0.0];
        let b = vec![0.0f32, 1.0];
        assert!((cosine_similarity(&a, &b)).abs() < 1e-6);
    }

    #[test]
    fn cosine_similarity_zero_vector() {
        let a = vec![0.0f32, 0.0];
        let b = vec![1.0f32, 1.0];
        assert_eq!(cosine_similarity(&a, &b), 0.0);
    }

    #[test]
    fn rrf_merge_single_list() {
        let bm25 = vec![("a".to_string(), 0.9f32), ("b".to_string(), 0.5)];
        let merged = rrf_merge(bm25, vec![], 60);
        assert_eq!(merged, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn rrf_merge_overlap_boosts_score() {
        let bm25 = vec![("a".to_string(), 0.9f32), ("b".to_string(), 0.5)];
        let vector = vec![("b".to_string(), 0.95f32), ("c".to_string(), 0.8)];
        let merged = rrf_merge(bm25, vector, 60);
        // b appears in both lists — should rank first
        assert_eq!(merged[0], "b");
    }
}
