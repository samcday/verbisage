---

## Assistant (Plan · unsloth/Qwen3.6-27B-GGUF · 3.5s)

_Thinking:_

Let me read the sqlite build functions:


**Tool: read**

**Input:**
```json
{
  "filePath": "/projects/llm_swapspace/verbisage/src/backends/build.rs",
  "limit": 200,
  "offset": 148
}
```

**Output:**
```
<path>/projects/llm_swapspace/verbisage/src/backends/build.rs</path>
<type>file</type>
<content>
148: fn build_sqlite(
149:     def: &ResolvedBackendDef,
150:     lang: &str,
151:     lp: &LanguagePaths,
152: ) -> (
153:     Box<dyn DictionaryBackend>,
154:     Option<Box<dyn SpellChecker>>,
155:     Option<Box<dyn Predictor>>,
156: ) {
157:     let files = resolve_files(def, lang, lp);
158:     let path = files.first().cloned();
159: 
160:     if path.is_none() {
161:         eprintln!(
162:             "warning: no sqlite database found for '{}' (tried: {})",
163:             lang,
164:             files
165:                 .iter()
166:                 .map(|p| p.display().to_string())
167:                 .collect::<Vec<_>>()
168:                 .join(", ")
169:         );
170:     }
171: 
172:     let has_dict = def.capabilities.contains(&Capability::Dictionary)
173:         || def.capabilities.contains(&Capability::Unigrams);
174:     let has_ngrams = def.capabilities.contains(&Capability::Ngrams);
175: 
176:     let path = match path {
177:         Some(p) => p,
178:         None => {
179:             let empty: Box<dyn DictionaryBackend> = Box::new(FileDictionaryBackend::new());
180:             return (empty, None, None);
181:         }
182:     };
183: 
184:     if has_dict && has_ngrams {
185:         // Share connection between dict and predictor
186:         match SharedSqliteConnection::open(&path) {
187:             Ok(shared) => {
188:                 eprintln!("info: loaded sqlite database: {}", path.display());
189:                 let table_ngrams = def.table_ngrams.as_deref().unwrap_or("ngrams");
190:                 let next_col = def.next_col.as_deref().unwrap_or("next");
191:                 let freq_col = def.freq_col.as_deref().unwrap_or("frequency");
192:                 let context_cols = if def.context_cols.is_empty() {
193:                     vec!["prev".to_string()]
194:                 } else {
195:                     def.context_cols.clone()
196:                 };
197:                 let max_order = context_cols.len();
198:                 let writable = true;
199: 
200:                 let dict = if def.table.is_some() {
201:                     // Separate dictionary table
202:                     let table = def.table.as_deref().unwrap_or("words");
203:                     let word_col = def.word_col.as_deref().unwrap_or("word");
204:                     SqliteDictionaryBackend::from_shared(
205:                         shared.clone(),
206:                         table,
207:                         word_col,
208:                         freq_col,
209:                         writable,
210:                     )
211:                 } else {
212:                     // Shared-table mode: dict reads ngram table's unigram rows
213:                     SqliteDictionaryBackend::from_ngram_unigrams(
214:                         shared.clone(),
215:                         table_ngrams,
216:                         &context_cols,
217:                         next_col,
218:                         freq_col,
219:                         writable,
220:                     )
221:                 };
222: 
223:                 let sc: Box<dyn SpellChecker> =
224:                     Box::new(SqliteSpellChecker::new(Arc::new(dict.clone())));
225: 
226:                 let backend = SqliteNgramBackend::new(
227:                     shared,
228:                     table_ngrams,
229:                     &context_cols,
230:                     next_col,
231:                     freq_col,
232:                     max_order,
233:                     writable,
234:                 );
235:                 let predictor = SmoothedPredictor::new(Box::new(backend));
236: 
237:                 (Box::new(dict), Some(sc), Some(Box::new(predictor)))
238:             }
239:             Err(e) => {
240:                 eprintln!(
241:                     "warning: failed to open sqlite db '{}': {}",
242:                     path.display(),
243:                     e
244:                 );
245:                 let empty: Box<dyn DictionaryBackend> = Box::new(FileDictionaryBackend::new());
246:                 (empty, None, None)
247:             }
248:         }
249:     } else if has_dict {
250:         // Dict only (or dict + unigrams)
251:         let _writable = true; // user file — TODO: determine writability
252:         let result = SqliteDictionaryBackend::from_sqlite(
253:             &path,
254:             def.table.as_deref().unwrap_or("words"),
255:             def.word_col.as_deref().unwrap_or("word"),
256:             def.freq_col.as_deref().unwrap_or("frequency"),
257:             true,
258:         );
259:         match result {
260:             Ok(dict) => {
261:                 let sc: Box<dyn SpellChecker> =
262:                     Box::new(SqliteSpellChecker::new(Arc::new(dict.clone())));
263:                 (Box::new(dict), Some(sc), None)
264:             }
265:             Err(e) => {
266:                 eprintln!(
267:                     "warning: failed to open sqlite db '{}': {}",
268:                     path.display(),
269:                     e
270:                 );
271:                 let empty: Box<dyn DictionaryBackend> = Box::new(FileDictionaryBackend::new());
272:                 (empty, None, None)
273:             }
274:         }
275:     } else if has_ngrams {
276:         // Ngrams only
277:         let table_ngrams = def.table_ngrams.as_deref().unwrap_or("ngrams");
278:         let next_col = def.next_col.as_deref().unwrap_or("next");
279:         let freq_col = def.freq_col.as_deref().unwrap_or("frequency");
280:         let context_cols = if def.context_cols.is_empty() {
281:             vec!["prev".to_string()]
282:         } else {
283:             def.context_cols.clone()
284:         };
285:         let max_order = context_cols.len();
286:         match SqliteNgramBackend::from_path(
287:             &path,
288:             table_ngrams,
289:             &context_cols,
290:             next_col,
291:             freq_col,
292:             max_order,
293:             true,
294:         ) {
295:             Ok(backend) => {
296:                 let predictor = SmoothedPredictor::new(Box::new(backend));
297:                 let empty: Box<dyn DictionaryBackend> = Box::new(FileDictionaryBackend::new());
298:                 (empty, None, Some(Box::new(predictor)))
299:             }
300:             Err(e) => {
301:                 eprintln!(
302:                     "warning: failed to open ngram predictor '{}': {}",
303:                     path.display(),
304:                     e
305:                 );
306:                 let empty: Box<dyn DictionaryBackend> = Box::new(FileDictionaryBackend::new());
307:                 (empty, None, None)
308:             }
309:         }
310:     } else {
311:         let empty: Box<dyn DictionaryBackend> = Box::new(FileDictionaryBackend::new());
312:         (empty, None, None)
313:     }
314: }
315: 
316: #[cfg(not(feature = "sqlite"))]
317: fn build_sqlite(
318:     _def: &ResolvedBackendDef,
319:     _lang: &str,
320:     _lp: &LanguagePaths,
321: ) -> (
322:     Box<dyn DictionaryBackend>,
323:     Option<Box<dyn SpellChecker>>,
324:     Option<Box<dyn Predictor>>,
325: ) {
326:     eprintln!("warning: sqlite feature not enabled");
327:     (Box::new(FileDictionaryBackend::new()), None, None)
328: }
329: 
330: // ---------------------------------------------------------------------------
331: // Marisa backend (placeholder — uses FileDictionaryBackend)
332: // ---------------------------------------------------------------------------
333: 
334: #[cfg(feature = "marisa")]
335: fn build_marisa(
336:     def: &ResolvedBackendDef,
337:     lang: &str,
338:     lp: &LanguagePaths,
339: ) -> (
340:     Box<dyn DictionaryBackend>,
341:     Option<Box<dyn SpellChecker>>,
342:     Option<Box<dyn Predictor>>,
343: ) {
344:     use std::sync::Arc;
345: 
346:     let trie_candidates = resolve_marisa_ngram_trie_files(def, lang, lp);
347:     let counts_candidates = resolve_marisa_ngram_counts_files(def, lang, lp);

(Showing lines 148-347 of 752. Use offset=348 to continue.)
</content>
```

---

## Assistant (Plan · unsloth/Qwen3.6-27B-GGUF · 11.6s)

_Thinking:_

Now I have a clear picture. Let me look at the ngram backend trait to understand what operations we need to support:


**Tool: read**

**Input:**
```json
{
  "filePath": "/projects/llm_swapspace/verbisage/src/prediction/ngram_backend.rs",
  "limit": 50,
  "offset": 1
}
```

**Output:**
```
<path>/projects/llm_swapspace/verbisage/src/prediction/ngram_backend.rs</path>
<type>file</type>
<content>
1: /// Trait that abstracts the n‑gram data source.
2: ///
3: /// Implementors provide raw counts and candidate words without any scoring
4: /// logic.  The [`SmoothedPredictor`](crate::prediction::smoothed::SmoothedPredictor)
5: /// consumes this trait to compute interpolated probabilities.
6: pub trait NgramBackend: Send + Sync {
7:     /// Maximum n‑gram order supported (e.g., 3 for a trigram model).
8:     fn max_order(&self) -> usize;
9: 
10:     /// Total sum of all unigram counts (for the unigram denominator).
11:     fn unigram_total(&self) -> u64;
12: 
13:     /// Exact count for a given n‑gram (ordered sequence of words).
14:     ///
15:     /// Returns 0 if the n‑gram is not found.
16:     fn ngram_count(&self, ngram: &[&str]) -> u64;
17: 
18:     /// Gather candidate continuation words for a given context.
19:     ///
20:     /// The context is the last `order-1` words.  Returns `(word, count)`
21:     /// pairs sorted by count descending, limited to `max_candidates`.
22:     fn candidates(&self, context: &[&str], max_candidates: usize) -> Vec<(String, u64)>;
23: 
24:     /// Whether this backend supports write operations.
25:     fn is_writable(&self) -> bool {
26:         false
27:     }
28: 
29:     /// Increase the frequency of an n-gram by `delta`.
30:     ///
31:     /// `ngram` is the full sequence including context and next word
32:     /// (e.g., `["hello", "world"]` for bigram "hello world").
33:     /// For unigrams, `ngram` is `["world"]`.
34:     ///
35:     /// `save_unknown`: if true, create the n-gram with frequency = `delta`
36:     /// when it doesn't exist; if false, return Err for unknown n-grams.
37:     fn increase_ngram_frequency(
38:         &self,
39:         _ngram: &[&str],
40:         _delta: f64,
41:         _save_unknown: bool,
42:     ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
43:         Err("not supported".into())
44:     }
45: }
46: 
47: impl<T: NgramBackend + ?Sized> NgramBackend for std::sync::Arc<T> {
48:     fn max_order(&self) -> usize {
49:         self.as_ref().max_order()
50:     }

(Showing lines 1-50 of 77. Use offset=51 to continue.)
</content>
```

---

## Assistant (Plan · unsloth/Qwen3.6-27B-GGUF · 5.9s)

_Thinking:_

Now I have a full picture. Let me also check the dictionary backend trait:


**Tool: read**

**Input:**
```json
{
  "filePath": "/projects/llm_swapspace/verbisage/src/dictionary/mod.rs",
  "limit": 50,
  "offset": 70
}
```

**Output:**
```
<path>/projects/llm_swapspace/verbisage/src/dictionary/mod.rs</path>
<type>file</type>
<content>
70:     /// lexicographically by word.
71:     fn query_prefixes(&self, queries: &[DictionaryQuery]) -> Vec<DictionaryResult>;
72: 
73:     /// Retrieve the normalised frequency for a word (0.0–1.0).
74:     fn get_frequency(&self, word: &str) -> f64;
75: 
76:     /// Return `true` when the word exists in the dictionary.
77:     fn contains(&self, word: &str) -> bool;
78: 
79:     /// Whether this backend supports write operations.
80:     fn is_writable(&self) -> bool {
81:         false
82:     }
83: 
84:     /// Add a word to the dictionary.
85:     ///
86:     /// For backends that store frequency, `frequency` is used.
87:     /// For backends that only track membership (MARISA, Hunspell),
88:     /// `frequency` is ignored.
89:     ///
90:     /// `allow_existing`: if true, overwrite when the word exists;
91:     /// if false, return Err when the word already exists.
92:     fn add_word(
93:         &self,
94:         _word: &str,
95:         _frequency: f64,
96:         _allow_existing: bool,
97:     ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
98:         Err("not supported".into())
99:     }
100: }
101: 
102: impl<T: DictionaryBackend> DictionaryBackend for Arc<T> {
103:     fn query_prefixes(&self, queries: &[DictionaryQuery]) -> Vec<DictionaryResult> {
104:         (**self).query_prefixes(queries)
105:     }
106: 
107:     fn get_frequency(&self, word: &str) -> f64 {
108:         (**self).get_frequency(word)
109:     }
110: 
111:     fn contains(&self, word: &str) -> bool {
112:         (**self).contains(word)
113:     }
114: 
115:     fn is_writable(&self) -> bool {
116:         (**self).is_writable()
117:     }
118: 
119:     fn add_word(

(Showing lines 70-119 of 380. Use offset=120 to continue.)
</content>
```

---

## Assistant (Plan · unsloth/Qwen3.6-27B-GGUF · 5.6s)

_Thinking:_

Now let me check the dictionary sqlite add_word:


**Tool: read**

**Input:**
```json
{
  "filePath": "/projects/llm_swapspace/verbisage/src/dictionary/sqlite.rs",
  "limit": 80,
  "offset": 318
}
```

**Output:**
```
<path>/projects/llm_swapspace/verbisage/src/dictionary/sqlite.rs</path>
<type>file</type>
<content>
318:     fn add_word(
319:         &self,
320:         word: &str,
321:         frequency: f64,
322:         allow_existing: bool,
323:     ) -> Result<(), SharedError> {
324:         if !self.writable {
325:             return Err("backend is not writable".into());
326:         }
327: 
328:         if self.ngram_unigram_mode {
329:             self.add_word_ngram_unigram(word, frequency, allow_existing)
330:         } else {
331:             self.add_word_simple(word, frequency, allow_existing)
332:         }
333:     }
334: }
335: 
336: impl SqliteDictionaryBackend {
337:     // -- Simple mode implementations --
338: 
339:     fn query_prefixes_simple(&self, queries: &[DictionaryQuery]) -> Vec<DictionaryResult> {
340:         self.cache.get_or_compute(queries, |queries| {
341:             if queries.is_empty() {
342:                 return Vec::new();
343:             }
344: 
345:             let mut clauses = Vec::new();
346:             for q in queries {
347:                 let mut cond = Vec::new();
348: 
349:                 if let Some(ref prefix) = q.prefix {
350:                     cond.push(format!("{} LIKE '{}%'", self.word_column, prefix));
351:                 }
352:                 if let Some(ref suffix) = q.suffix {
353:                     cond.push(format!("{} LIKE '%{}'", self.word_column, suffix));
354:                 }
355:                 if let Some(min) = q.min_length {
356:                     cond.push(format!("LENGTH({}) >= {}", self.word_column, min));
357:                 }
358:                 if let Some(max) = q.max_length {
359:                     if max != usize::MAX {
360:                         cond.push(format!("LENGTH({}) <= {}", self.word_column, max));
361:                     }
362:                 }
363: 
364:                 let clause = if cond.is_empty() {
365:                     "1".to_string()
366:                 } else {
367:                     cond.join(" AND ")
368:                 };
369:                 clauses.push(format!("({})", clause));
370:             }
371: 
372:             let sql = format!(
373:                 "SELECT {}, {} FROM {} WHERE {}",
374:                 self.word_column,
375:                 self.frequency_column,
376:                 self.table_name,
377:                 clauses.join(" OR ")
378:             );
379: 
380:             let mut all_results = Vec::new();
381:             let conn = self.conn.lock();
382:             match conn.prepare(&sql) {
383:                 Ok(mut stmt) => {
384:                     if let Ok(rows) = stmt.query_map([], |row| {
385:                         let word: String = row.get(0)?;
386:                         let frequency = match row.get::<_, f64>(1) {
387:                             Ok(f) => f,
388:                             Err(_) => {
389:                                 let int_freq: i64 = row.get(1)?;
390:                                 int_freq as f64
391:                             }
392:                         };
393:                         Ok(DictionaryResult {
394:                             word,
395:                             confidence: if frequency > 0.0 { frequency } else { -1.0 },
396:                         })
397:                     }) {

(Showing lines 318-397 of 896. Use offset=398 to continue.)
</content>
```

---

## Assistant (Plan · unsloth/Qwen3.6-27B-GGUF · 140.1s)

_Thinking:_

Now I have a clear picture. Let me analyze the problem:

## Current State

**Presage actual schema** (from `/usr/share/presage/database_en.db`):
- `_1_gram`: `(word TEXT, count INTEGER, UNIQUE(word))`
- `_2_gram`: `(word_1 TEXT, word TEXT, count INTEGER, UNIQUE(word_1, word))`
- `_3_gram`: `(word_2 TEXT, word_1 TEXT, word TEXT, count INTEGER, UNIQUE(word_2, word_1, word))`

**Current code expects** (from `SqlitePreset::Presage`):


- `table_ngrams: "ngrams"`, `context_cols: ["prev"]`, `next_col: "next"`, `freq_col: "frequency"`

**The mismatch:**
1. Presage uses per-order tables (`_1_gram`, `_2_gram`, `_3_gram`) while the code assumes a single `ngrams` table
2. Column names differ: Presage uses `word`, `word_1`, `word_2`, and `count`, but the code expects `prev`, `next`, and `frequency`

The `SqliteNgramBackend` currently reads from one table with a context column for unigrams (NULL) and a next_word column for bigrams. To support Presage's per-order tables, I need to either modify the backend to query multiple tables based on ngram order or create a new Presage-specific variant. I think the cleanest approach is to update `SqliteNgramBackend` to handle both schemas natively, since the real Presage format uses `_1_gram`, `_2_gram`, `_3_gram` tables with `count` columns instead of the current `ngrams` table with `prev`/`next`/`frequency`.

The user wants all sqlite backends to use the Presage format, so I should update the `SqlitePreset::Presage` to match this actual schema and modify `SqliteNgramBackend` to dispatch queries to the appropriate table based on ngram order. I could track the tables as a vector and detect what's available, or store the table pattern like `_N_gram` and generate table names dynamically.

I'm thinking about whether to keep a single `SqliteNgramBackend` that handles both per-order and single-table schemas, or split them into separate backends—consolidating seems cleaner since we'd just detect which tables exist and route queries accordingly.

For the backend's internal structure, I'm considering storing the ngram tables as a map or ordered collection so we can quickly look up the right table for a given n-gram order.

I'm weighing whether to store a table pattern like `_N_gram` that gets resolved dynamically, or explicitly map each order to its table name—the explicit map feels cleaner for validation. Then I need to think through how the `marisa_sqlite` backend should work as a composite type, where the system data comes from marisa for dictionary and ngrams, while user data flows through sqlite for mutations.

The user wants `marisa_sqlite` to be a hybrid backend with a `user_backend` key that references another backend definition, resolving with the same logic as any other backend reference. The Presage schema uses per-order tables with specific column names like `_1_gram`, `_2_gram`, etc., where each table has a word column and a count column, and for n-grams, the context words precede the final word column.

For the dictionary backend, it queries `_1_gram` for words and frequencies, while the ngram backend routes queries to the appropriate table based on the n-gram order. When mutations happen through `MergedDictionary`, it finds the first writable backend, and `MergedPredictor` tries all predictors until one succeeds. The user's config defines a `marisa_sqlite` backend type with a `user_backend` reference to a sqlite backend that handles user data.

I'm thinking through the implementation steps: first, update `SqliteNgramBackend` to handle per-order tables, then fix the preset to match the actual Presage schema, add the new backend type, and handle the build function. But I'm realizing there's a question about whether the current approach of separating `SqliteDictionaryBackend` and `SqliteNgramBackend` is still the right way to go.

Looking at how Presage structures things, both dictionary and ngram queries ultimately pull from the same `_1_gram` table, just with different query patterns. The dictionary does prefix searches while the ngrams do exact lookups and candidate retrieval. So they're really just different query modes on the same underlying data rather than fundamentally separate concerns.

I think the best approach is to keep them as separate structs but have them share a single connection when they're reading from the same database. The real changes needed are updating the Presage preset to use the correct table and column names, then refactoring the ngram backend to properly handle per-order tables instead of relying on a single table.

For the user's custom backend type, they want a composite backend that combines marisa and sqlite backends, where the marisa backend serves as the system dictionary and ngrams, and the sqlite backend serves as the user dictionary and ngrams. I should implement this as a new backend type that accepts both `system_backend` and `user_backend` parameters and resolves them like other backend references.

Now I'm reconsidering the table structure. The Presage format uses separate tables for each n-gram order—`_1_gram`, `_2_gram`, `_3_gram`—with specific column layouts where the last column is always `count` and the preceding columns are `word_N` for context words. This is different from a single table approach, and I need to account for this multi-table structure in the implementation.

The current `SqliteNgramBackend` is designed around a single table with fixed context and next word columns, but Presage requires routing queries to different tables based on n-gram order. I'm weighing whether to support both formats through configuration or stick with just Presage since that's what the user wants.

The cleanest approach seems to be adding a `per_order` flag that switches between the old single-table mode and the new Presage per-order mode, or alternatively supporting multiple `table_ngrams` entries mapped to different orders. I'm leaning toward the flag approach as the simplest solution. I think the cleanest approach is to support both schemas natively by having `SqliteNgramBackend` detect which format it's dealing with based on configuration. If `per_order_tables` is set to true, it queries separate `_1_gram`, `_2_gram`, `_3_gram` tables following the Presage naming convention; otherwise it falls back to the current single-table approach. The config can specify this flag along with the table naming pattern, so a Presage backend would set `per_order_tables = true` with the standard `_N_gram` pattern, while a custom backend could use either format.

Now I'm realizing there's a fundamental mismatch between how the current backend works and what Presage expects—the backend uses a single table with context and next word columns, but Presage uses separate tables per order with explicit word columns for each position. I need to decide whether to make the backend flexible enough to handle both formats, or just switch entirely to the Presage format since that's what the user actually needs.

The cleanest approach is to support both: add a `per_order_tables` flag that determines which query strategy to use, then update the builder to detect which tables exist and configure the backend accordingly. I'll also add a `presage` format preset that sets this flag and configures the correct column names.

For the `marisa_sqlite` backend type, I need to figure out how it should work...

it's a composite backend that resolves two separate backend references—one for the system trie and one for user sqlite data—and combines them into a merged result with the system backend providing the read-only dictionary and ngrams, while the user backend handles dictionary additions, ngram frequency updates, and spellchecking.

I think the cleanest approach is to add `marisa_sqlite` as a new backend type that resolves its two internal backends and builds both, then merges their outputs together rather than treating it as a chain assignment.

For the Presage schema, the key insight is that the `word` column serves dual purposes—it's the word itself for dictionary lookups and the "next word" for n-gram predictions, with unigrams stored in `_1_gram` and longer n-grams in higher-order tables. I need to configure the preset to use the correct table names (`_1_gram` for dict/unigrams, `_2_gram` for bigrams, etc.) with `word` as the column name and `count` as the frequency column, while updating the ngram backend to route queries to the appropriate table based on order.

Now I'm considering whether to keep the dictionary and ngram backends separate or consolidate them into a single component, since they're currently distinct but the schema suggests they could work together more tightly.

I think keeping them separate makes sense—the dictionary handles prefix queries and spellchecking while the ngram backend manages predictions, and they can share a single connection when needed. The key is that both should query from the same `_1_gram` table for unigrams, which aligns with how the existing code already structures this.

Now I need to outline the implementation plan, but first I should clarify the `marisa_sqlite` backend with the user—specifically whether the user sqlite backend should auto-create its database if it doesn't exist, or if we should error out like we do for other backends.

I also need to understand how the `marisa_sqlite` backend resolves to the right language's databases. The system marisa backend and user sqlite backend each have their own path resolution, so I'm thinking the user sqlite should use a single database file per verbisage instance with words tagged by language, or alternatively use the same per-language pattern but with sqlite instead of marisa.

Looking at the user's comment about the same naming scheme and paths, it seems like the user sqlite should follow the same pattern as the system marisa backend—so if the system backend uses `./marisa_ngrams/database_{lang}/ngrams.trie`, the user backend would use something like `~/.local/share/verbisage/user_{lang}.db`. The user sqlite should auto-create on first write, following Presage schema, and I need to clarify how the `marisa_sqlite` backend resolves to the right language's databases.

I think the cleanest approach is to present the plan and ask for confirmation on the key design points before proceeding.

Now I'm reconsidering the sqlite backend architecture. The current `SqlitePreset::Presage` format doesn't match the actual Presage schema, so I need to fix that. But more importantly, I'm thinking about whether we should support multiple sqlite formats at all. The user wants Presage as the standard format, so maybe the right approach is to make Presage the default and only support custom overrides when needed, rather than having multiple preset formats. I'm realizing the `SqlitePreset::Presage` was never actually correct—it assumed a table structure that doesn't match the actual Presage schema at all. So I need to fix the preset to use the real table names and columns, then update the `SqliteNgramBackend` to handle the per-order tables properly, and finally add the `marisa_sqlite` backend to tie everything together. I'm mapping out the Presage schema where each n-gram order gets its own table with word columns in sequence, and I'll default to the `_N_gram` naming convention with `word` and `count` columns, allowing users to override the pattern if needed.

For the implementation, I need to refactor `SqliteNgramBackend` to store a table name resolver function that generates the right table name based on the n-gram order, then update the query methods to use this resolver instead of a static table name.

I'm weighing whether to keep the old single-table format alongside the new per-order format or just transition to per-order tables entirely—the user seems to want to standardize on Presage format, so I could simplify by dropping support for the old format, though keeping both for backward compatibility might be worth the extra complexity.

The cleanest approach is to support both through a mode field in the resolved backend definition, where the builder detects which tables exist and sets the mode accordingly. For the config, I'd add a `per_order_tables` flag to the backend definition and update the preset to enable it, then modify the backend to dispatch queries based on which mode it's in.

I think I have enough clarity now to outline the full implementation plan.


Here's the situation clearly:

**Presage actual schema** (from `database_en.db`):

| Table | Columns | Role |
|---|---|---|
| `_1_gram` | `word TEXT, count INTEGER` | Dictionary + Unigrams |
| `_2_gram` | `word_1 TEXT, word TEXT, count INTEGER` | Bigrams |
| `_3_gram` | `word_2 TEXT, word_1 TEXT, word TEXT, count INTEGER` | Trigrams |

**What the code currently expects** (broken preset):

| Field | Current value | Should be |
|---|---|---|
| `table_ngrams` | `"ngrams"` | per-order tables |
| `context_cols` | `["prev"]` | `["word_1"]`, `["word_2", "word_1"]` |
| `next_col` | `"next"` | `"word"` |
| `freq_col` | `"frequency"` | `"count"` |

The `SqliteNgramBackend` is built around a **single table** with context columns that are NULL for unigrams. Presage uses **separate tables per order**. The backend needs to fundamentally change for Presage format.

Here's the plan:

### Phase 1: Fix `SqliteNgramBackend` for Presage schema

**A. Add `per_order_tables` mode to `SqliteNgramBackend`**

The backend needs to route queries to the right table based on n-gram order. Two approaches:

1. **Mode flag** - `per_order_tables: bool` + `table_pattern: String` (e.g. `_N_gram`) where `N` gets substituted. Old single-table mode stays as fallback.
2. **Table map** - `tables: HashMap<usize, String>` mapping order to table name. More explicit, no pattern substitution.

I'd lean toward **option 1** (pattern + flag) since it's one config field and handles any number of orders naturally. The `ResolvedBackendDef` gets a `per_order_tables: bool` field.

**B. Update `SqlitePreset::Presage`**

```
per_order_tables: true
table_pattern: "_N_gram"  (auto-substitutes _1_gram, _2_gram, etc.)
freq_col: "count"
dict_table: "_1_gram"   (for dict queries)
dict_word_col: "word"
```

**C. Update `build_sqlite`**

When `per_order_tables` is true:
- Dict backend queries `_1_gram` with `word`/`count`
- Ngram backend gets the pattern, routes by order
- They share a `SharedSqliteConnection`

### Phase 2: `marisa_sqlite` backend type

New `BackendType::MarisaSqlite` that resolves two sub-backends and composes them:

```toml
[backends.main]
type = "marisa_sqlite"
system_backend = "default_marisa"   # read-only trie for dict/ngrams
user_backend = "user_presage"       # writable sqlite for mutations

[backends.user_presage]
type = "sqlite"
format = "presage"
user_dir = "~/.local/share/verbisage"
user_patterns = ["user_{lang}.db"]
```

The `build_marisa_sqlite` function:
1. Resolves `system_backend` -> builds marisa backend (dict + predictor from trie)
2. Resolves `user_backend` -> builds sqlite backend (dict from `_1_gram` + predictor from `_N_gram` tables)
3. Merges: `MergedDictionary([marisa_dict, sqlite_dict])`, `MergedPredictor([marisa_pred, sqlite_pred])`
4. Word-add goes to sqlite (first writable), ngram-bump goes to sqlite (first success)

### Questions before I proceed:

1. **Dict vs Ngram backend separation** - For Presage, both dict and ngrams read from the same DB, just different tables (`_1_gram` for dict, `_2_gram`+ for ngrams). Should I merge them into a single `PresageSqliteBackend` struct, or keep the current `SqliteDictionaryBackend` + `SqliteNgramBackend` separation (just sharing a connection)? My read: **keep them separate** - the current separation is clean; they just share a `SharedSqliteConnection` which already works.

2. **DB auto-creation for user sqlite** - When the user sqlite DB doesn't exist (first run), should it auto-create with the Presage schema (`_1_gram`, `_2_gram`, `_3_gram` tables)? Or error out and require manual creation?

3. **Old single-table format** - Should we keep supporting the old `ngrams` / `prev` / `next` / `frequency` format for custom backends, or drop it entirely since the user wants Presage everywhere?

---

## User

note that for using sqlite as a dict, the _1_gram table serves as our dictionary.

---

## User

we do *not* want to use a separate sqlite file and/or table format for dict, it's supposed to all come from the one file (two if there's a system file but you know what i mean...)

---

## Assistant (Plan · unsloth/Qwen3.6-27B-GGUF · 53.9s)

_Thinking:_

The user is clarifying that for Presage format, there's only ONE file with ONE format. The `_1_gram` table serves as both dictionary AND unigram source. There's no separate dict table.

Let me re-read their full intent:

1. All sqlite backends use Presage format (`_1_gram`, `_2_gram`, `_3_gram` tables)
2. The dict comes from `_1_gram` (word, count) — same table that serves unigrams
3. There's no separate "dict table" vs "ngram table" concept for Presage
4. The `marisa_sqlite` backend type combines a read-only system backend (marisa trie) with a writable user backend (sqlite presage)

So the current `SqliteDictionaryBackend` and `SqliteNgramBackend` separation is fine architecturally, but both need to be rewritten for the Presage schema:


- `SqliteDictionaryBackend` reads from `_1_gram(word, count)` and needs to handle the `count` column as integers
- `SqliteNgramBackend` needs to route queries to the correct table based on n-gram order, using `_1_gram`, `_2_gram`, or `_3_gram`

The key issue is that `SqliteNgramBackend` currently assumes a single table with configurable context columns, but Presage uses separate tables per order, so I need to add routing logic to direct each query to the appropriate table.

For the dictionary side, when `per_order_tables` is enabled, the backend should query `_1_gram` directly rather than using unigram mode. I'm also realizing the user wants a unified approach where the Presage format applies across all SQLite backends, with both dictionary and ngram backends reading from the same database file using the standard column names (`word`, `count`, and context columns like `word_1`, `word_2`).

The `marisa_sqlite` backend type chains together a read-only marisa trie for system data with a writable sqlite presage backend for user data, where mutations route to the sqlite backend while dictionary and prediction queries merge results from both sources. Before diving into implementation, I should clarify whether the old custom format should be dropped entirely or kept as a fallback, and whether we need automatic database creation for user sqlite files. I need to address the user's clarification about the unified file approach and then answer the remaining questions about auto-creation and format support. The user already made it clear that everything should come from one file, so I should acknowledge that and move forward with clarifying the auto-creation behavior and whether we need to support the old format alongside the new one.


Understood — one Presage file, `_1_gram` serves as dictionary, unigrams, and part of the ngram chain. No separate dict table.

To

---

## User

as for your questions: 1. i think sharing them could make more sense?
2. when the user db doesnt exist, it should be created lazily on write (add_word or ngram update)
3. drop it entirely

---

## Assistant (Plan · unsloth/Qwen3.6-27B-GGUF · 8.5s)

_Thinking:_

Let me make sure I understand all the requirements clearly:

1. **Presage schema is the only sqlite format** - `_1_gram(word, count)`, `_2_gram(word_1, word, count)`, `_3_gram(word_2, word_1, word, count)`
2. **One DB file, one set of tables** - `_1_gram` serves as dict + unigrams, `_2_gram`/`_3_gram` for higher-order ngrams
3. **Merge dict and ngram backends** into a single `PresageSqliteBackend` that implements both `DictionaryBackend` and `NgramBackend` (wrapped in `SmoothedPredictor`)
4. **Lazy DB creation** - user DB auto-creates on first write
5. **Drop old single-table format entirely**

---

## User

you got it, write that to plan6.md
additional note: we only want to run CREATE TABLE or similar if we just created the entire file; if the file already exists, we do *not* modify the schema!

