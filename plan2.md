# verbisage — Multi-source backend design

## 1. Config format

```toml
backend = "my_full"                     # chain: 1-3 segments, `+` separated
# backend = "my_dict+my_freq+my_lm"    # 3 segments
# backend = "my_dict+my_lm"            # 2 segments

[backends.my_full]
type = "sqlite"
path = "data_{lang}.db"
format = "presage"

[backends.my_dict]
type = "file"
format = "flat"

[backends.my_lm]
type = "sqlite"
path = "lm_{lang}.db"
format = "presage_ngrams"

[backends.my_freq]
type = "file"
format = "freq"
path = "{lang}.freq"

[backends.custom_db]
type = "sqlite"
path = "custom_{lang}.db"
format = "custom"
enable_unigrams = true
table = "my_words"
word_col = "term"
freq_col = "score"

[backends.marisa_trie]
type = "marisa"
path = "{lang}.marisa"

[backends.hunspell_en]
type = "hunspell"
```

## 2. Backend types

| Type | Default file patterns (LanguagePaths) | Backend trait impl |
|------|---------------------------------------|-------------------|
| `file` | `{lang}.dic`, `{lang}.freq`, `{lang}.wordlist` | `FileDictionaryBackend` + csv |
| `sqlite` | `database_{lang}.db`, `lm_{lang}.db` | `SqliteDictionaryBackend` / `SqlitePredictor` |
| `marisa` | `{lang}.marisa` | `MarisaDictionaryBackend` (new, via `rsmarisa`) |
| `hunspell` | resolved via `zspell` tag | `HunspellDictionaryBackend` (new) |

Each named backend resolves its own file independently: pattern scan by default, or direct `path` (with `{lang}` expansion) if set.

## 3. Format presets per type

### file presets

| Preset | delimiter | word_index | freq_index | Capabilities |
|--------|-----------|------------|------------|-------------|
| `flat` | None | None | None | dict |
| `freq` | None (whitespace) | Some(0) | Some(1) | dict + unigrams |
| `csv_unigrams` | None (must set by user) | Some(0) | Some(1) | dict + unigrams |
| `custom` | None | None | None | depends on user config |

Validation: `delimiter = None + any index set` → error. No delimiter + no indexes → flat mode.

### sqlite presets

| Preset | Dict+Unigram schema | Ngram schema | Capabilities |
|--------|--------------------|--------------|-------------|
| `presage_words` | `table=words, word_col=word, freq_col=frequency` | — | dict + unigrams |
| `presage_unigrams` | `table=unigrams, word_col=word, freq_col=frequency` | — | unigrams only |
| `presage_ngrams` | — | `table=ngrams, context_cols=[prev], next_col=next, freq_col=frequency` | ngrams only |
| `presage` | same as `presage_words` | same as `presage_ngrams` | dict + unigrams + ngrams |
| `custom` | all None | all None | depends on user config |

`enable_unigrams` / `enable_ngrams` override preset defaults. If enabled but required fields still None after preset+user merge → config error.

## 4. Chain semantics

```
parse("A+B+C") → ["A", "B", "C"]

1 segment:  backend capabilities dict ∩ unigrams ∩ ngrams — inherit all
2 segments: seg0 = dict, seg1 = seg1's capabilities ∩ {unigrams, ngrams}
3 segments: seg0 = dict, seg1 = unigrams, seg2 = ngrams
```

### Validation rules

**1 segment:**
- Empty capabilities → **error**
- Has freqs (uni/ngram) but no dict → **warn**

**2 segments:**
- seg0 must provide dict → **error** if not
- seg1 provides neither uni nor ngram → **warn** (redundant)
- seg1 capabilities beyond its role → **warn** if it provides dict (ignored)
- If seg1 provides only ngrams → unigrams absent (fine)
- If seg1 provides only unigrams → ngrams absent (fine)

**3 segments:**
- seg0 must provide dict → **error** if not
- seg1 should provide unigrams → **warn** if not
- seg2 should provide ngrams → **warn** if not
- Extra capabilities on seg1/seg2 beyond assigned role → **warn** ("ignored")

## 5. Backend building

```
resolve_backend(def, lang, paths):
  1. Determine resolved path(s):
     - If def.path set → expand {lang}, use directly
     - Else → use LanguagePaths pattern scan for def.type
  2. For each resolved file:
     - Match def.type to concrete backend impl
     - Apply format preset defaults + user overrides
     - Validate required schema fields vs capabilities
     - Instantiate backend (with SQLite connection sharing)
  3. Return (DictionaryBackend, Option<Predictor>, Option<SpellChecker>)
```

### SQLite connection sharing

If a single `[backend.X]` has both unigrams+ngrams enabled, one `Arc<Mutex<Connection>>` is shared between `SqliteDictionaryBackend` and `SqlitePredictor`.

Newtype: `SharedSqliteConnection(Arc<Mutex<Connection>>)`. Dedup by resolved path within a single backend def (not across different named backends).

## 6. Composite backends

### MergedDictionary (implements DictionaryBackend)

Wraps `Vec<Box<dyn DictionaryBackend>>`:
- `query_prefixes`: union results from all inner backends, dedup by word, first-source confidence wins
- `contains`: true if any inner backend has the word
- `get_frequency`: first non-zero from inner backends (priority order: dict backend first, then frequency-specific)

### MergedPredictor (implements Predictor)

Wraps `Vec<Box<dyn Predictor>>`:
- `predict_next`: query each inner predictor, merge by word (first-source wins), sort by confidence desc

### Spell-checker selection

First available from dict backends in priority:
1. Hunspell-based backend (HunspellSpellChecker)
2. Sqlite-based backend (SqliteSpellChecker)
3. File-based backend (DictionarySpellChecker wrapping the dict backend)
4. None → fallback to dict `contains` + prefix suggestions

## 7. CachedBackend construction

```
compose_chain(assignment):
  dict_sources = [backend for role == dict]
  freq_layer = [backend for role == unigrams]  # merged into MergedDictionary
  ngram_sources = [backend.predictor for role == ngrams]

  CachedBackend {
    loaded: true,
    dictionary: MergedDictionary(dict_sources ∪ freq_layer),
    predictor: ngram_sources.is_empty() ? None : MergedPredictor(ngram_sources),
    spellchecker: select_spellchecker(dict_sources),
  }
```

## 8. Backward compat

`backend = "file"` with no `[backends.file]` → auto-generate implicit `BackendDef { type=file, format=flat, ... }` using old pattern defaults.

Same for `"sqlite"` and `"hunspell"`.

## 9. Implementation order

1. `src/backends/` module — `BackendDef`, `BackendType`, format presets, config parsing, validation
2. Chain parsing (`parse_chain`, `assign_roles`) with validation rules
3. Format preset tables + user-config merge logic
4. SQLite connection sharing (`SharedSqliteConnection`)
5. Builders (`build_backend` per type, `compose_chain`)
6. `MergedDictionary` + `MergedPredictor` composite impls
7. Wire into `DaemonHandler::build_backend()`, CLI `open_backend()`, config `apply_defaults()`
8. New backends: `MarisaDictionaryBackend`, `HunspellDictionaryBackend`, CSV format in file backend
