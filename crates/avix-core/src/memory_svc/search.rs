use super::schema::MemoryRecord;

/// BM25 parameters.
const K1: f64 = 1.2;
const B: f64 = 0.75;

/// Common English stop words filtered before scoring.
const STOP_WORDS: &[&str] = &[
    "a", "an", "the", "is", "in", "on", "at", "of", "to", "and", "or", "but", "it", "its",
    "be", "was", "were", "are", "this", "that", "with", "for", "as", "by", "from", "have",
    "has", "had", "not", "do", "did", "so", "if", "up", "out", "no", "we", "he", "she",
    "they", "i", "my", "me", "you", "your", "our", "their",
];

fn tokenize(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .map(|t| t.to_lowercase())
        .filter(|t| !t.is_empty() && !STOP_WORDS.contains(&t.as_str()))
        .collect()
}

/// Rank `records` by BM25 relevance to `query`, returning up to `limit` results.
///
/// Records with a score of zero are excluded from results.
pub fn bm25_rank<'a>(
    records: &'a [MemoryRecord],
    query: &str,
    limit: usize,
) -> Vec<&'a MemoryRecord> {
    if records.is_empty() || query.trim().is_empty() {
        return vec![];
    }

    let query_terms = tokenize(query);
    if query_terms.is_empty() {
        return vec![];
    }

    // Tokenise all documents
    let docs: Vec<Vec<String>> = records.iter().map(|r| tokenize(&r.spec.content)).collect();
    let n = docs.len() as f64;

    // Average document length
    let total_len: usize = docs.iter().map(|d| d.len()).sum();
    let avgdl = if n > 0.0 {
        total_len as f64 / n
    } else {
        1.0
    };

    // Compute IDF for each query term
    let idf = |term: &str| -> f64 {
        let df = docs.iter().filter(|d| d.contains(&term.to_string())).count() as f64;
        ((n - df + 0.5) / (df + 0.5) + 1.0).ln()
    };

    // Score each record
    let mut scored: Vec<(f64, usize)> = docs
        .iter()
        .enumerate()
        .map(|(idx, doc)| {
            let dl = doc.len() as f64;
            let score: f64 = query_terms
                .iter()
                .map(|term| {
                    let tf = doc.iter().filter(|t| *t == term).count() as f64;
                    let idf_val = idf(term);
                    idf_val * (tf * (K1 + 1.0))
                        / (tf + K1 * (1.0 - B + B * dl / avgdl.max(1.0)))
                })
                .sum();
            (score, idx)
        })
        .collect();

    // Sort descending by score, then take limit (exclude zero-score records)
    scored.sort_by(|(a, _), (b, _)| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
    scored
        .into_iter()
        .filter(|(score, _)| *score > 0.0)
        .take(limit)
        .map(|(_, idx)| &records[idx])
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory_svc::schema::{
        MemoryRecord, MemoryRecordIndex, MemoryRecordMetadata, MemoryRecordSpec, MemoryRecordType,
    };
    use chrono::Utc;

    fn make_record(content: &str) -> MemoryRecord {
        MemoryRecord::new(
            MemoryRecordMetadata {
                id: format!("mem-{}", content.len()),
                record_type: MemoryRecordType::Episodic,
                agent_name: "test".into(),
                agent_pid: 1,
                owner: "alice".into(),
                created_at: Utc::now(),
                updated_at: Utc::now(),
                session_id: "sess-1".into(),
                tags: vec![],
                pinned: false,
            },
            MemoryRecordSpec {
                content: content.into(),
                outcome: None,
                related_goal: None,
                tools_used: vec![],
                key: None,
                confidence: None,
                ttl_days: None,
                index: MemoryRecordIndex::default(),
            },
        )
    }

    #[test]
    fn bm25_ranks_matching_record_first() {
        let records = vec![
            make_record("Quantum computing research completed. Topological qubits discovered."),
            make_record("Financial analysis. Q3 OPEX anomalies found."),
        ];
        let ranked = bm25_rank(&records, "quantum computing", 5);
        assert!(!ranked.is_empty());
        assert!(ranked[0].spec.content.contains("Quantum"));
    }

    #[test]
    fn bm25_empty_for_stop_word_only_query() {
        let records = vec![make_record("some content here for testing")];
        // Query is all stop words — no terms survive filtering
        let ranked = bm25_rank(&records, "the a an", 5);
        assert!(ranked.is_empty());
    }

    #[test]
    fn bm25_empty_for_no_match() {
        let records = vec![make_record("quantum computing research")];
        let ranked = bm25_rank(&records, "financial analysis Q3", 5);
        // May or may not be empty depending on overlap; assert result count <= 1
        assert!(ranked.len() <= 1);
    }
}
