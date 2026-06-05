# Plan 4: Port Presage Smoothed N-gram Predictors

## Goal

Port the Presage **linear interpolation smoothed n-gram prediction** algorithm to verbisage, making it work across **both** SQLite and MARISA backends via a unified trait. Currently our `MarisaPredictor` and `SqlitePredictor` do naive single-order MLE (maximum likelihood estimation) without smoothing. The Presage algorithm combines unigram, bigram, and trigram probabilities via configurable interpolation weights (deltas).

## The Algorithm (from both presage1 and presage2)

For a candidate next-word `w_i` given context tokens, the smoothed probability is:

```
P(w_i | context) = Σ_{k=0}^{cardinality-1} delta_k * frequency_k

frequency_0 = count(w_i) / unigram_counts_sum
frequency_k = count(context[-(k-1)..], w_i) / count(context[-(k-1)..])   for k > 0
```

Where:
- `delta_k` = interpolation weight for order k (default: `0.01 0.1 0.89` for trigram)
- `count(ngram)` = exact count of that n-gram from the backend
- `unigram_counts_sum` = total count of all unigrams
- If `count(ngram)` = 0 (not found), that order contributes 0
- If `denominator <= 0` or `denominator < numerator`, frequency = 0 (sanity guard)

### Two-phase prediction:
1. **Candidate gathering** — backoff from highest order to unigram, collecting candidate next-words via prefix search. Stop when we have enough candidates.
2. **Scoring** — for each candidate, compute the full smoothed probability across all orders.

## Presage Data Format Details

### Presage1 (SQLite)
- Separate tables per order: `_1_gram`, `_2_gram`, `_3_gram`
- Schema: `word_N TEXT, ..., word_1 TEXT, word TEXT, count INTEGER`
- `getUnigramCountsSum()` = `SELECT SUM(count) FROM _1_gram`
- `getNgramCount(ngram)` = exact lookup from the right table
- `getNgramLikeTable(ngram, limit)` = LIKE query on the `word` column for candidate gathering

### Presage2 (MARISA trie)
- Same n-gram key format: `N w1 w2 ... wN`
- Counts file: **int32_t** array, memory-mapped
  - `count_data[0]` = unigram_counts_sum (stored explicitly)
  - `count_data[1 + trie_id]` = count for n-gram at that trie ID
- `getPredictedWords(ngram, filter, threshold, limit)` — prefix search with bounded top-k

### Our current format
- MARISA: same key format as presage2, but counts file has 4-byte magic header + raw u32 array indexed directly by trie ID (no unigram sum at index 0)
- SQLite: single `ngrams` table with `context_1`, `context_2`, `next_word`, `frequency` columns — different from presage1's per-order tables

## Notes

- No backward-compat needed — no external dependents on the concrete `MarisaPredictor` / `SqlitePredictor` types.
- `src/prediction/marisa.rs` and `src/prediction/sqlite.rs` are **replaced in place** with the new `NgramBackend` implementations (same filenames, new content).
- `SmoothedPredictor` and `NgramBackend` are wired through `build.rs` to replace the old predictors entirely.

## Design

### Step 1: Define `NgramBackend` trait

New trait in `src/prediction/ngram_backend.rs` that abstracts the n-gram data source:

```rust
pub trait NgramBackend: Send + Sync {
    /// Maximum n-gram order supported (e.g., 3 for trigram model).
    fn max_order(&self) -> usize;

    /// Total sum of all unigram counts (for unigram denominator).
    fn unigram_total(&self) -> u64;

    /// Exact count for a given n-gram (ordered sequence of words).
    /// Returns 0 if the n-gram is not found.
    fn ngram_count(&self, ngram: &[&str]) -> u64;

    /// Gather candidate continuation words for a given context prefix.
    /// The context is the last `order-1` words. Returns (word, count) pairs
    /// sorted by count descending, limited to `max_candidates`.
    fn candidates(&self, context: &[&str], max_candidates: usize) -> Vec<(String, u64)>;
}
```

### Step 2: Implement `NgramBackend` for MARISA

Replaces `src/prediction/marisa.rs` — new struct `MarisaNgramBackend`:
- Loads the same `.trie` + `.counts` files as `MarisaPredictor`
- `max_order()`: determined by scanning the trie for highest order prefix found
- `unigram_total()`: sum of all "1 " prefixed keys' counts
- `ngram_count(ngram)`: lookup key `"{order} {w1} {w2} ..."` in trie
- `candidates(context, limit)`: prefix search on `"{order} {context} "` key, extract next words, aggregate counts, return top-k

Key difference from current `MarisaPredictor`: this is a **data access layer**, not a predictor. It provides raw counts and candidates without any scoring logic.

### Step 3: Implement `NgramBackend` for SQLite

Replaces `src/prediction/sqlite.rs` — new struct `SqliteNgramBackend`:
- Wraps `SharedSqliteConnection`
- `max_order()`: configurable at construction
- `unigram_total()`: `SELECT SUM(frequency) FROM ngrams WHERE context_1 IS NULL` (or whatever our schema uses)
- `ngram_count(ngram)`: query the right columns for exact match
- `candidates(context, limit)`: query with LIKE/prefix match, return top-k

Adapts to our existing single-table schema (not presage1's per-order tables).

### Step 4: Implement `SmoothedPredictor`

New struct in `src/prediction/smoothed.rs`:

```rust
pub struct SmoothedPredictor {
    backend: Box<dyn NgramBackend>,
    deltas: Vec<f64>,       // interpolation weights [delta_0, delta_1, ...]
    count_threshold: u64,   // minimum count to consider a candidate
}
```

Two-phase algorithm:
1. **Gather candidates** — iterate from highest order to 1, calling `backend.candidates()` at each order, deduplicating words, stopping when we have enough.
2. **Score candidates** — for each candidate word `w_i`, compute:
   ```
   prob = 0
   for k in 0..deltas.len():
       ngram = context[k..] + [w_i]  // order k+1 n-gram
       numerator = backend.ngram_count(ngram)
       denominator = if k == 0 { backend.unigram_total() }
                      else { backend.ngram_count(&context[k..]) }
       freq = if denominator > 0 && denominator >= numerator {
                   numerator as f64 / denominator as f64
               } else { 0.0 }
       prob += deltas[k] * freq
   ```
3. Return predictions sorted by probability descending, filtered to `probability > 0`.

The `SmoothedPredictor` implements `Predictor` and delegates `predict_next()` to the smoothed algorithm.

### Step 5: Integration

- Default deltas: `0.01, 0.1, 0.89` (trigram, matching Presage)
- The `build.rs` backend builder should construct `SmoothedPredictor` wrapping the appropriate `NgramBackend` when the backend has `Ngrams` capability
- Old `MarisaPredictor` and `SqlitePredictor` are replaced by `SmoothedPredictor` + `NgramBackend` implementations

## Implementation Order

1. Create `NgramBackend` trait (`src/prediction/ngram_backend.rs`)
2. Replace `src/prediction/marisa.rs` with `MarisaNgramBackend` implementation
3. Replace `src/prediction/sqlite.rs` with `SqliteNgramBackend` implementation
4. Create `SmoothedPredictor` (`src/prediction/smoothed.rs`)
5. Update `prediction/mod.rs` to wire everything together
6. Update `build.rs` to use new predictors
7. Update tests

## Constraints
- `cargo fmt` before every commit
- Small incremental commits, code files only
- Confidence values must be in 0.0-1.0 range (or at least non-negative and bounded by sum of deltas)
