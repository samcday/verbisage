# Ngram ngram-trie predictor data merge

The key insight is that system and user data are NOT merged at the database level. Instead, they're merged at the *prediction* level:

1. Two separate predictor instances are configured:
   - `DefaultSmoothedNgramTriePredictor` - read-only, system language model
   - `UserSmoothedNgramPredictor` - read-write, user learning model

2. Each predictor queries its own database independently and produces a `Prediction` (list of `Suggestion` objects with probabilities)

3. The `MeritocracyCombiner` then merges all predictions by:
   - Collecting all suggestions from all predictors
   - Passing them through a `filter()` that deduplicates by token and accumulates probabilities

So the "on-the-fly merging" is: two independent n-gram models predict separately, then their probability-weighted suggestions are combined by accumulating probabilities for matching tokens.

---

# How Presage Ngram Predictors Merge System and User Data

## High-Level Architecture

System and user data are **not merged at the database level**. Instead, Presage uses **two independent predictor instances**, each querying its own separate database, and merges their results **at prediction time** via a combiner. This is the "on-the-fly" merging.

## Configuration

From `packaging/sailfish/presage.xml`, the active predictor pipeline is:

```xml
<PREDICTORS>DefaultSmoothedNgramTriePredictor UserSmoothedNgramPredictor DefaultHunspellPredictor DefaultRecencyPredictor</PREDICTORS>
```

Two n-gram predictors are defined with distinct databases:

| Predictor | Class | Database | Mode | LEARN |
|---|---|---|---|---|
| `DefaultSmoothedNgramTriePredictor` | `SmoothedNgramTriePredictor` | `/usr/share/presage/database_empty` (MARISA trie) | read-only | N/A |
| `UserSmoothedNgramPredictor` | `SmoothedNgramPredictor` | `~/.local/share/presage/lm.db` (SQLite) | read-write | `true` |

Both share the same delta weights: `0.01 0.1 0.89` (trigram model: 1% unigram, 10% bigram, 89% trigram).

## Prediction Flow

### Step 1: PredictorActivator iterates all predictors

In `predictorActivator.cpp:57-91`:

```cpp
PredictorRegistry::Iterator it = predictorRegistry->iterator();
while (it.hasNext()) {
    predictor = it.next();
    predictions.push_back(predictor->predict(max_partial_prediction_size * multiplier, filter));
}
result = combiner->combine(predictions);
```

Each predictor gets the **same context** (from the shared `ContextTracker`) and returns a `Prediction` (a list of `Suggestion` objects, each with a token string and a probability).

### Step 2: Each predictor computes probabilities independently

Both `SmoothedNgramPredictor` and `SmoothedNgramTriePredictor` use the same **Kneser-Ney-style linear interpolation** formula. For each candidate word `w_i`:

```
P(w_i | context) = Σ_k δ_k × Count(w_{i-k}...w_i) / Count(w_{i-k}...w_{i-1})
```

Where `δ_k` are the deltas (0.01, 0.1, 0.89). Each predictor queries its own database for counts. The trie predictor queries the MARISA trie + mmap'd counts; the SQLite predictor queries SQL tables.

### Step 3: MeritocracyCombiner merges predictions

In `meritocracyCombiner.cpp:44-55`:

```cpp
Prediction MeritocracyCombiner::combine(const std::vector<Prediction>& predictions) {
    Prediction result;
    for (each prediction) {
        for (each suggestion in prediction) {
            result.addSuggestion(suggestion);  // just concatenate
        }
    }
    return filter(result);  // deduplicate + accumulate
}
```

### Step 4: filter() deduplicates and accumulates probabilities

In `combiner.cpp:55-98`:

```cpp
Prediction Combiner::filter(const Prediction& prediction) const {
    for (each suggestion i) {
        token = suggestion.getWord();
        if (token not yet seen) {
            for (each later suggestion j) {
                if (prediction[j].word == token) {
                    // accumulate probability, capped at MAX_PROBABILITY
                    suggestion.setProbability(suggestion.prob + prediction[j].prob);
                }
            }
            result.addSuggestion(suggestion);
        }
    }
    return result;
}
```

**This is the merge**: if the system model and user model both suggest "the" with probabilities 0.3 and 0.5, the merged result has "the" with probability 0.8 (capped at 1.0).

## Key Design Points

1. **Separate databases, separate predictors**: System model is a pre-built, read-only MARISA trie. User model is a writable SQLite database. They are never joined at the storage layer.

2. **Merge is probability accumulation**: The combiner doesn't weight sources differently. A suggestion from the user model and one from the system model with the same token get their probabilities summed.

3. **No cross-database queries**: Each predictor's `predict()` call is entirely self-contained against its own database. The merge happens purely on the `Prediction` objects in memory.

4. **The trie predictor is read-only by design** (`TrieDatabaseConnector` throws if `read_write` is true). Learning only goes to the SQLite user database.

5. **Order matters for tie-breaking**: The `filter()` keeps the first occurrence of a token and accumulates duplicates into it. So predictors listed earlier in the PREDICTORS config get priority for the "base" entry.

## Reimplementation Checklist

- **Two language models**: One large static corpus (system), one small dynamic corpus (user).
- **Same interpolation formula**: Both use the same delta weights so their probabilities are comparable.
- **Independent prediction**: Each model produces its own ranked suggestions.
- **Post-hoc merge**: Concatenate all suggestions, deduplicate by token, sum probabilities.
- **Cap at 1.0**: Accumulated probability is clamped to `MAX_PROBABILITY` (1.0).
- **Online learning**: Only the user model accepts `learn()` calls. The system model is immutable.

---

# How N-gram Frequencies Are Adjusted (Learning)

## Adding a New Word (n-gram does not exist)

When a new n-gram is encountered, it's inserted with an initial count of **1** (the observed frequency in the current learning event):

```cpp
// smoothedNgramPredictor.cpp:457-461
int count = db->getNgramCount(ngram);
if (count > 0) {
    // existing n-gram
} else {
    // n-gram not in database, insert it
    db->insertNgram(ngram, it->second);  // it->second = observed frequency (typically 1)
}
```

SQL: `INSERT INTO _N_gram VALUES(..., 1);`

**Critical**: the check is `if (count > 0)`. `getNgramCount()` returns `0` when the row doesn't exist (empty result set). This means an n-gram with an actual stored count of `0` would be misclassified as "not found" and trigger a duplicate-key INSERT error. In practice this doesn't matter because counts always start at `1` and only increase, but it's worth noting the assumption.

## Marking an Existing Word as Used (frequency increase)

When the n-gram already exists, its count is incremented by simple addition:

```cpp
int count = db->getNgramCount(ngram);
if (count > 0) {
    db->updateNgram(ngram, count + it->second);  // new = old + observed
    check_learn_consistency(ngram);
}
```

SQL: `UPDATE _N_gram SET count = old_count + freq WHERE ...;`

## Consistency Enforcement

After updating an existing n-gram, `check_learn_consistency()` ensures that higher-order n-grams never exceed their sub-n-gram counts. If a trigram's count surpasses a contained bigram's count, the bigram is bumped by exactly **+1** via `incrementNgramCount()`.

## Why the Trie Can't Support Learning

The trie-based predictor (`SmoothedNgramTriePredictor`) has empty stubs for `learn()` and `forget()`, and `TrieDatabaseConnector` throws if constructed with `read_write = true`. Three reasons:

1. **MARISA tries are immutable by design** — built in one shot from a `Keyset`, then saved. No API to insert keys into an existing trie; you'd have to rebuild the entire structure.

2. **Fixed-size counts array** — the counts file is a flat `int32[]` mmap'd with `PROT_READ`. Can't grow or modify it in place. Adding a new n-gram would require a new slot, meaning reallocation.

3. **System model by design** — it's a large pre-built corpus baked into the package. Rebuilding on every user keystroke would defeat the performance benefit of the trie + mmap approach.

**Conclusion**: tries can only be used read-only as system files and must be combined with an SQLite backend for user data.

## Prediction Probability Formula (for reference)

The raw counts feed into the smoothed interpolation formula at prediction time:

```
P(word | context) = Σ_k δ_k × count(ngram_k) / count(prefix_k)
```

So increasing a count directly increases the numerator, raising the word's probability proportionally relative to its prefix count.

---
