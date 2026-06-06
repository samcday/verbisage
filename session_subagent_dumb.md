# SQLite Presage Backend Rewrite

**Session ID:** ses_1655b941affeGmN8LlEa61QIBv
**Created:** 6/6/2026, 1:55:27 AM
**Updated:** 6/6/2026, 2:04:30 AM

---

## User

please follow @plan6.md 

---

## Assistant (Build · unsloth/Qwen3.6-27B-GGUF · 543.5s)

_Thinking:_

The user wants me to follow plan6.md. Let me first understand the codebase structure, then start implementing the plan phases.

Let me start by exploring the codebase to understand the current state.


I'll start by exploring the codebase structure to understand the current state before implementing the plan.



**Tool: task**

**Input:**
```json
{
  "description": "Explore codebase structure",
  "prompt": "Explore the codebase at /projects/llm_swapspace/verbisage thoroughly. I need to understand:\n\n1. The overall project structure - list all files in src/ recursively\n2. The current SQLite backend implementations - find all files related to SQLite backends, dictionary backends, and ngram backends\n3. The current build.rs in src/backends/ - read it fully\n4. The BackendDef and ResolvedBackendDef types - find and read their definitions\n5. The current SqlitePreset, SqliteDictionaryBackend, SqliteNgramBackend - find and read their definitions\n6. The DictionaryBackend and NgramBackend traits - find and read them\n7. The SmoothedPredictor - find and read it\n8. The MergedDictionary and MergedPredictor - find and read them\n9. The SqliteSpellChecker - find and read it\n10. Any existing marisa backend implementations\n\nReturn the full content of all key files and a summary of the project structure. Be very thorough - read every relevant file completely.",
  "subagent_type": "explore"
}
```

**Output:**
```
<task id="ses_1655b18ffffeDiOvSfxLvUq6F5" state="completed">
<task_result>
I have completed the full codebase exploration. Here is what was accomplished:

## Completed

1. **Full file inventory** -- Listed all 38+ files across the entire `src/` tree, covering backends, clients, config, daemon, debug, dictionary, prediction, and spellcheck modules.

2. **Read and documented all key files** -- Every core source file was read in full, including:
   - `backends/mod.rs`, `build.rs`, `chain.rs`, `merged.rs`, `shared_sqlite.rs`
   - `dictionary/mod.rs`, `sqlite.rs`, `file.rs`, `marisa.rs`, `hunspell.rs`, `paths.rs`, `compact.rs`, `subsequence.rs`
   - `prediction/mod.rs`, `ngram_backend.rs`, `smoothed.rs`, `sqlite.rs`, `marisa.rs`, `frequency.rs`
   - `spellcheck/mod.rs`, `sqlite.rs`, `dictionary.rs`, `hunspell.rs`
   - `lib.rs`, `config.rs`, `cli.rs`, `Cargo.toml`
   - Daemon files: `daemon/mod.rs`, `config.rs`, `dbus.rs`, `handlers.rs`, `protocol.rs`, `stdio.rs`
   - Client files: `clients/mod.rs`, `stdio.rs`, `dbus.rs`
   - Binaries: `bin/verbisage.rs`, `bin/verbisaged.rs`, `bin/verbisage-introspect.rs`

3. **Documented all traits, types, and their implementations** -- Including `DictionaryBackend`, `NgramBackend`, `Predictor`, `SpellChecker`, `BackendDef`, `ResolvedBackendDef`, `SqlitePreset`, `SmoothedPredictor`, `MergedDictionary`, `MergedPredictor`, `SharedSqliteConnection`, and the full build/chain composition pipeline.

4. **Mapped out the architecture** -- Feature flags, trait hierarchy, backend construction flow, chain parsing logic, merging strategy, and smoothing algorithm.

---

There are no remaining exploration steps. How would you like to proceed? For example:

1. **Deep dive into a specific module** (e.g., daemon protocol, D-Bus integration, CLI argument handling)?
2. **Identify gaps or missing implementations** (e.g., what's not yet wired up, TODO items)?
3. **Analyze test coverage** or look for tests?
4. **Something else entirely** -- what is the goal you have in mind for this codebase?
</task_result>
</task>
```

---

## Assistant (Build · unsloth/Qwen3.6-27B-GGUF · 21.8s)

_Thinking:_

The exploration agent completed but didn't give me the actual file contents. I need to read the key files myself to understand the current implementation before making changes. Let me read the critical files.


Let me read the key source files directly to understand the current implementation.



**Tool: read**

**Input:**
```json
{
  "filePath": "/projects/llm_swapspace/verbisage/src/dictionary/sqlite.rs"
}
```

**Output:**
```
<path>/projects/llm_swapspace/verbisage/src/dictionary/sqlite.rs</path>
<type>file</type>
<content>
1: use std::error::Error;
2: use std::path::Path;
3: 
4: use crate::backends::SharedSqliteConnection;
5: 
6: use super::{DictionaryBackend, DictionaryQuery, DictionaryResult, SharedQueryCache};
7: 
8: type SharedError = Box<dyn Error + Send + Sync>;
9: 
10: /// Dictionary backend backed by a SQLite table.
11: ///
12: /// Two modes:
13: ///
14: /// 1. **Simple mode** (default): queries a `(word, frequency)` table directly.
15: ///    Use `from_sqlite`, `from_sqlite_readonly`, `from_shared`, or `new`.
16: ///
17: /// 2. **Ngram unigram mode**: queries the ngram table's unigram rows
18: ///    (`context IS NULL`) instead of a dedicated words table. Use
19: ///    `from_ngram_unigrams`. This is the correct mode for Presage SQLite
20: ///    data where the `_1_gram` table is both the dictionary and the
21: ///    unigram frequency source.
22: ///
23: /// # Thread safety
24: ///
25: /// The inner [`rusqlite::Connection`] is wrapped in a [`SharedSqliteConnection`]
26: /// so that the backend implements [`Sync`].  The connection can be shared
27: /// with a [`SmoothedPredictor`](crate::prediction::smoothed::SmoothedPredictor) backed
28: /// by a [`SqliteNgramBackend`](crate::prediction::sqlite::SqliteNgramBackend) when both
29: /// unigrams and n-grams come from the same database file.
30: pub struct SqliteDictionaryBackend {
31:     conn: SharedSqliteConnection,
32:     table_name: String,
33:     word_column: String,
34:     frequency_column: String,
35:     cache: SharedQueryCache,
36:     writable: bool,
37:     /// When true, queries the ngram table's unigram rows (context IS NULL)
38:     /// instead of a dedicated (word, frequency) table.
39:     ngram_unigram_mode: bool,
40:     /// Column names for the ngram table (only used when ngram_unigram_mode is true).
41:     ngram_context_columns: Vec<String>,
42:     ngram_next_word_column: String,
43: }
44: 
45: impl SqliteDictionaryBackend {
46:     /// Open a SQLite database at `path` with the given writability.
47:     pub fn from_sqlite<P: AsRef<Path>>(
48:         path: P,
49:         table_name: &str,
50:         word_column: &str,
51:         frequency_column: &str,
52:         writable: bool,
53:     ) -> Result<Self, SharedError> {
54:         Ok(Self {
55:             conn: SharedSqliteConnection::open(path.as_ref())?,
56:             table_name: table_name.to_string(),
57:             word_column: word_column.to_string(),
58:             frequency_column: frequency_column.to_string(),
59:             cache: SharedQueryCache::new(),
60:             writable,
61:             ngram_unigram_mode: false,
62:             ngram_context_columns: Vec::new(),
63:             ngram_next_word_column: String::new(),
64:         })
65:     }
66: 
67:     /// Open a SQLite database **read-only** (system dictionaries).
68:     pub fn from_sqlite_readonly<P: AsRef<Path>>(
69:         path: P,
70:         table_name: &str,
71:         word_column: &str,
72:         frequency_column: &str,
73:     ) -> Result<Self, SharedError> {
74:         Self::from_sqlite(path, table_name, word_column, frequency_column, false)
75:     }
76: 
77:     /// Wrap a shared connection (allows sharing with a predictor).
78:     pub fn from_shared(
79:         conn: SharedSqliteConnection,
80:         table_name: &str,
81:         word_column: &str,
82:         frequency_column: &str,
83:         writable: bool,
84:     ) -> Self {
85:         Self {
86:             conn,
87:             table_name: table_name.to_string(),
88:             word_column: word_column.to_string(),
89:             frequency_column: frequency_column.to_string(),
90:             cache: SharedQueryCache::new(),
91:             writable,
92:             ngram_unigram_mode: false,
93:             ngram_context_columns: Vec::new(),
94:             ngram_next_word_column: String::new(),
95:         }
96:     }
97: 
98:     /// Create an in-memory database (`:memory:`).
99:     pub fn new() -> Self {
100:         Self {
101:             conn: SharedSqliteConnection::in_memory(),
102:             table_name: "words".to_string(),
103:             word_column: "word".to_string(),
104:             frequency_column: "frequency".to_string(),
105:             cache: SharedQueryCache::new(),
106:             writable: true,
107:             ngram_unigram_mode: false,
108:             ngram_context_columns: Vec::new(),
109:             ngram_next_word_column: String::new(),
110:         }
111:     }
112: 
113:     /// Create a backend that queries the ngram table's unigram rows.
114:     ///
115:     /// This is the correct constructor for Presage SQLite data where the
116:     /// unigram table is both the dictionary and the unigram frequency source.
117:     pub fn from_ngram_unigrams(
118:         conn: SharedSqliteConnection,
119:         table_name: &str,
120:         context_columns: &[String],
121:         next_word_column: &str,
122:         frequency_column: &str,
123:         writable: bool,
124:     ) -> Self {
125:         Self {
126:             conn,
127:             table_name: table_name.to_string(),
128:             word_column: next_word_column.to_string(),
129:             frequency_column: frequency_column.to_string(),
130:             cache: SharedQueryCache::new(),
131:             writable,
132:             ngram_unigram_mode: true,
133:             ngram_context_columns: context_columns.to_vec(),
134:             ngram_next_word_column: next_word_column.to_string(),
135:         }
136:     }
137: 
138:     /// Check whether the expected table exists in the database.
139:     pub fn table_exists(&self) -> bool {
140:         let sql = "SELECT name FROM sqlite_master WHERE type='table' AND name=?1";
141:         let conn = self.conn.lock();
142:         conn.prepare(sql)
143:             .and_then(|mut stmt| stmt.exists([&self.table_name]))
144:             .unwrap_or(false)
145:     }
146: 
147:     /// Ensure the table and indexes exist (idempotent).
148:     fn ensure_table(&self) {
149:         let conn = self.conn.lock();
150:         let sql = format!(
151:             "CREATE TABLE IF NOT EXISTS {} ({} TEXT NOT NULL, {} REAL NOT NULL)",
152:             self.table_name, self.word_column, self.frequency_column,
153:         );
154:         if let Err(e) = conn.execute(&sql, []) {
155:             eprintln!("warning: sqlite ensure_table failed — {}", e);
156:             return;
157:         }
158:         let idx_word = format!("idx_{}_{}", self.table_name, self.word_column);
159:         let _ = conn.execute(
160:             &format!(
161:                 "CREATE INDEX IF NOT EXISTS {} ON {}({})",
162:                 idx_word, self.table_name, self.word_column,
163:             ),
164:             [],
165:         );
166:         let idx_len = format!("idx_{}_{}_length", self.table_name, self.word_column);
167:         let _ = conn.execute(
168:             &format!(
169:                 "CREATE INDEX IF NOT EXISTS {} ON {}(LENGTH({}))",
170:                 idx_len, self.table_name, self.word_column,
171:             ),
172:             [],
173:         );
174:     }
175: 
176:     /// Return the frequency of `word`, or 0.0 if absent.
177:     pub fn frequency(&self, word: &str) -> f64 {
178:         if self.ngram_unigram_mode {
179:             self.frequency_ngram_unigram(word)
180:         } else {
181:             self.frequency_simple(word)
182:         }
183:     }
184: 
185:     fn frequency_simple(&self, word: &str) -> f64 {
186:         let sql = format!(
187:             "SELECT {} FROM {} WHERE {} = ?1 LIMIT 1",
188:             self.frequency_column, self.table_name, self.word_column
189:         );
190:         let conn = self.conn.lock();
191:         if let Ok(mut stmt) = conn.prepare(&sql) {
192:             if let Ok(mut rows) = stmt.query_map([word], |row| match row.get::<_, f64>(0) {
193:                 Ok(f) => Ok(f),
194:                 Err(_) => {
195:                     let int_freq: i64 = row.get(0)?;
196:                     Ok(int_freq as f64)
197:                 }
198:             }) {
199:                 if let Some(result) = rows.next() {
200:                     return result.unwrap_or(0.0);
201:                 }
202:             }
203:         }
204:         0.0
205:     }
206: 
207:     fn frequency_ngram_unigram(&self, word: &str) -> f64 {
208:         let null_conditions: Vec<String> = self
209:             .ngram_context_columns
210:             .iter()
211:             .map(|c| format!("{} IS NULL", c))
212:             .collect();
213:         let sql = format!(
214:             "SELECT {} FROM {} WHERE {} AND {} = ?1 LIMIT 1",
215:             self.frequency_column,
216:             self.table_name,
217:             null_conditions.join(" AND "),
218:             self.ngram_next_word_column
219:         );
220:         let conn = self.conn.lock();
221:         if let Ok(mut stmt) = conn.prepare(&sql) {
222:             if let Ok(mut rows) = stmt.query_map([word], |row| match row.get::<_, f64>(0) {
223:                 Ok(f) => Ok(f),
224:                 Err(_) => {
225:                     let int_freq: i64 = row.get(0)?;
226:                     Ok(int_freq as f64)
227:                 }
228:             }) {
229:                 if let Some(result) = rows.next() {
230:                     return result.unwrap_or(0.0);
231:                 }
232:             }
233:         }
234:         0.0
235:     }
236: 
237:     pub fn len(&self) -> usize {
238:         if self.ngram_unigram_mode {
239:             self.len_ngram_unigram()
240:         } else {
241:             self.len_simple()
242:         }
243:     }
244: 
245:     fn len_simple(&self) -> usize {
246:         let sql = format!("SELECT COUNT(*) FROM {}", self.table_name);
247:         let conn = self.conn.lock();
248:         if let Ok(mut stmt) = conn.prepare(&sql) {
249:             if let Ok(mut rows) = stmt.query_map([], |row| row.get::<_, i64>(0)) {
250:                 if let Some(result) = rows.next() {
251:                     return result.unwrap_or(0) as usize;
252:                 }
253:             }
254:         }
255:         0
256:     }
257: 
258:     fn len_ngram_unigram(&self) -> usize {
259:         let null_conditions: Vec<String> = self
260:             .ngram_context_columns
261:             .iter()
262:             .map(|c| format!("{} IS NULL", c))
263:             .collect();
264:         let sql = format!(
265:             "SELECT COUNT(*) FROM {} WHERE {}",
266:             self.table_name,
267:             null_conditions.join(" AND ")
268:         );
269:         let conn = self.conn.lock();
270:         if let Ok(mut stmt) = conn.prepare(&sql) {
271:             if let Ok(mut rows) = stmt.query_map([], |row| row.get::<_, i64>(0)) {
272:                 if let Some(result) = rows.next() {
273:                     return result.unwrap_or(0) as usize;
274:                 }
275:             }
276:         }
277:         0
278:     }
279: 
280:     pub fn is_empty(&self) -> bool {
281:         self.len() == 0
282:     }
283: 
284:     pub fn enable_cache(&self, size_limit: usize) {
285:         self.cache.set_limit(size_limit);
286:     }
287: 
288:     pub fn clear_cache(&self) {
289:         self.cache.clear();
290:     }
291: }
292: 
293: impl DictionaryBackend for SqliteDictionaryBackend {
294:     fn query_prefixes(&self, queries: &[DictionaryQuery]) -> Vec<DictionaryResult> {
295:         if self.ngram_unigram_mode {
296:             self.query_prefixes_ngram_unigram(queries)
297:         } else {
298:             self.query_prefixes_simple(queries)
299:         }
300:     }
301: 
302:     fn get_frequency(&self, word: &str) -> f64 {
303:         self.frequency(word)
304:     }
305: 
306:     fn contains(&self, word: &str) -> bool {
307:         if self.ngram_unigram_mode {
308:             self.contains_ngram_unigram(word)
309:         } else {
310:             self.contains_simple(word)
311:         }
312:     }
313: 
314:     fn is_writable(&self) -> bool {
315:         self.writable
316:     }
317: 
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
398:                         for row in rows.flatten() {
399:                             all_results.push(row);
400:                         }
401:                     }
402:                 }
403:                 Err(e) => {
404:                     eprintln!("warning: sqlite query failed — {}", e);
405:                 }
406:             }
407: 
408:             all_results.sort_by(|a, b| {
409:                 b.confidence
410:                     .partial_cmp(&a.confidence)
411:                     .unwrap_or(std::cmp::Ordering::Equal)
412:                     .then_with(|| a.word.cmp(&b.word))
413:             });
414: 
415:             all_results
416:         })
417:     }
418: 
419:     fn contains_simple(&self, word: &str) -> bool {
420:         let sql = format!(
421:             "SELECT 1 FROM {} WHERE {} = ?1 LIMIT 1",
422:             self.table_name, self.word_column
423:         );
424:         let conn = self.conn.lock();
425:         conn.prepare(&sql)
426:             .and_then(|mut stmt| stmt.exists([word]))
427:             .unwrap_or(false)
428:     }
429: 
430:     fn add_word_simple(
431:         &self,
432:         word: &str,
433:         frequency: f64,
434:         allow_existing: bool,
435:     ) -> Result<(), SharedError> {
436:         self.ensure_table();
437: 
438:         let sql = if allow_existing {
439:             format!(
440:                 "INSERT OR REPLACE INTO {} ({}, {}) VALUES (?1, ?2)",
441:                 self.table_name, self.word_column, self.frequency_column
442:             )
443:         } else {
444:             format!(
445:                 "INSERT INTO {} ({}, {}) VALUES (?1, ?2)",
446:                 self.table_name, self.word_column, self.frequency_column
447:             )
448:         };
449: 
450:         let conn = self.conn.lock();
451:         let result = conn.execute(&sql, [word, &frequency.to_string()]);
452: 
453:         if !allow_existing {
454:             if let Err(ref e) = result {
455:                 let err_str = e.to_string();
456:                 if err_str.contains("UNIQUE") || err_str.contains("unique") {
457:                     return Err(format!("word '{}' already exists", word).into());
458:                 }
459:             }
460:         }
461: 
462:         result.map(|_| ()).map_err(Into::into)
463:     }
464: 
465:     // -- Ngram unigram mode implementations --
466: 
467:     fn query_prefixes_ngram_unigram(&self, queries: &[DictionaryQuery]) -> Vec<DictionaryResult> {
468:         self.cache.get_or_compute(queries, |queries| {
469:             if queries.is_empty() {
470:                 return Vec::new();
471:             }
472: 
473:             let null_conditions: Vec<String> = self
474:                 .ngram_context_columns
475:                 .iter()
476:                 .map(|c| format!("{} IS NULL", c))
477:                 .collect();
478:             let base_where = null_conditions.join(" AND ");
479: 
480:             let mut clauses = Vec::new();
481:             for q in queries {
482:                 let mut cond = vec![base_where.clone()];
483: 
484:                 if let Some(ref prefix) = q.prefix {
485:                     cond.push(format!(
486:                         "{} LIKE '{}%'",
487:                         self.ngram_next_word_column, prefix
488:                     ));
489:                 }
490:                 if let Some(ref suffix) = q.suffix {
491:                     cond.push(format!(
492:                         "{} LIKE '%{}'",
493:                         self.ngram_next_word_column, suffix
494:                     ));
495:                 }
496:                 if let Some(min) = q.min_length {
497:                     cond.push(format!(
498:                         "LENGTH({}) >= {}",
499:                         self.ngram_next_word_column, min
500:                     ));
501:                 }
502:                 if let Some(max) = q.max_length {
503:                     if max != usize::MAX {
504:                         cond.push(format!(
505:                             "LENGTH({}) <= {}",
506:                             self.ngram_next_word_column, max
507:                         ));
508:                     }
509:                 }
510: 
511:                 let clause = cond.join(" AND ");
512:                 clauses.push(format!("({})", clause));
513:             }
514: 
515:             let sql = format!(
516:                 "SELECT {}, {} FROM {} WHERE {}",
517:                 self.ngram_next_word_column,
518:                 self.frequency_column,
519:                 self.table_name,
520:                 clauses.join(" OR ")
521:             );
522: 
523:             let mut all_results = Vec::new();
524:             let conn = self.conn.lock();
525:             match conn.prepare(&sql) {
526:                 Ok(mut stmt) => {
527:                     if let Ok(rows) = stmt.query_map([], |row| {
528:                         let word: String = row.get(0)?;
529:                         let frequency = match row.get::<_, f64>(1) {
530:                             Ok(f) => f,
531:                             Err(_) => {
532:                                 let int_freq: i64 = row.get(1)?;
533:                                 int_freq as f64
534:                             }
535:                         };
536:                         Ok(DictionaryResult {
537:                             word,
538:                             confidence: if frequency > 0.0 { frequency } else { -1.0 },
539:                         })
540:                     }) {
541:                         for row in rows.flatten() {
542:                             all_results.push(row);
543:                         }
544:                     }
545:                 }
546:                 Err(e) => {
547:                     eprintln!("warning: sqlite query failed — {}", e);
548:                 }
549:             }
550: 
551:             all_results.sort_by(|a, b| {
552:                 b.confidence
553:                     .partial_cmp(&a.confidence)
554:                     .unwrap_or(std::cmp::Ordering::Equal)
555:                     .then_with(|| a.word.cmp(&b.word))
556:             });
557: 
558:             all_results
559:         })
560:     }
561: 
562:     fn contains_ngram_unigram(&self, word: &str) -> bool {
563:         let null_conditions: Vec<String> = self
564:             .ngram_context_columns
565:             .iter()
566:             .map(|c| format!("{} IS NULL", c))
567:             .collect();
568:         let sql = format!(
569:             "SELECT 1 FROM {} WHERE {} AND {} = ?1 LIMIT 1",
570:             self.table_name,
571:             null_conditions.join(" AND "),
572:             self.ngram_next_word_column
573:         );
574:         let conn = self.conn.lock();
575:         conn.prepare(&sql)
576:             .and_then(|mut stmt| stmt.exists([word]))
577:             .unwrap_or(false)
578:     }
579: 
580:     fn add_word_ngram_unigram(
581:         &self,
582:         word: &str,
583:         frequency: f64,
584:         allow_existing: bool,
585:     ) -> Result<(), SharedError> {
586:         let context_placeholders: Vec<String> = self
587:             .ngram_context_columns
588:             .iter()
589:             .map(|_| "NULL".to_string())
590:             .collect();
591: 
592:         let columns = format!(
593:             "({}, {}, {})",
594:             self.ngram_context_columns.join(", "),
595:             self.ngram_next_word_column,
596:             self.frequency_column
597:         );
598:         let values = format!("({}, ?, ?)", context_placeholders.join(", "));
599: 
600:         let conn = self.conn.lock();
601: 
602:         if !allow_existing {
603:             // Rebuild with actual column names for context checks
604:             let exists_checks: Vec<String> = self
605:                 .ngram_context_columns
606:                 .iter()
607:                 .map(|col| format!("{} IS NULL", col))
608:                 .collect();
609:             let exists_sql = format!(
610:                 "SELECT 1 FROM {} WHERE {} = ? AND {}",
611:                 self.table_name,
612:                 self.ngram_next_word_column,
613:                 exists_checks.join(" AND ")
614:             );
615:             let exists: bool = conn
616:                 .query_row(&exists_sql, [word], |row| row.get(0))
617:                 .unwrap_or(false);
618:             if exists {
619:                 return Err(format!("word '{}' already exists", word).into());
620:             }
621:         }
622: 
623:         let sql = if allow_existing {
624:             format!(
625:                 "INSERT OR REPLACE INTO {} {} VALUES {}",
626:                 self.table_name, columns, values
627:             )
628:         } else {
629:             format!(
630:                 "INSERT INTO {} {} VALUES {}",
631:                 self.table_name, columns, values
632:             )
633:         };
634: 
635:         conn.execute(&sql, [word, &frequency.to_string()])
636:             .map(|_| ())
637:             .map_err(Into::into)
638:     }
639: }
640: 
641: impl Clone for SqliteDictionaryBackend {
642:     /// Returns a new backend sharing the same underlying connection.
643:     fn clone(&self) -> Self {
644:         Self {
645:             conn: self.conn.clone(),
646:             table_name: self.table_name.clone(),
647:             word_column: self.word_column.clone(),
648:             frequency_column: self.frequency_column.clone(),
649:             cache: SharedQueryCache::new(),
650:             writable: self.writable,
651:             ngram_unigram_mode: self.ngram_unigram_mode,
652:             ngram_context_columns: self.ngram_context_columns.clone(),
653:             ngram_next_word_column: self.ngram_next_word_column.clone(),
654:         }
655:     }
656: }
657: 
658: // ---------------------------------------------------------------------------
659: // Tests
660: // ---------------------------------------------------------------------------
661: 
662: #[cfg(test)]
663: mod tests {
664:     use super::*;
665:     use rusqlite::Connection;
666:     use tempfile::tempdir;
667: 
668:     #[test]
669:     fn basic_sqlite_operations() {
670:         let dir = tempdir().unwrap();
671:         let db_path = dir.path().join("test.db");
672: 
673:         {
674:             let conn = Connection::open(&db_path).unwrap();
675:             conn.execute(
676:                 "CREATE TABLE ngrams (word TEXT NOT NULL, frequency REAL NOT NULL)",
677:                 [],
678:             )
679:             .unwrap();
680:             conn.execute(
681:                 "INSERT INTO ngrams (word, frequency) VALUES (?1, ?2)",
682:                 ["hello", "100.0"],
683:             )
684:             .unwrap();
685:             conn.execute(
686:                 "INSERT INTO ngrams (word, frequency) VALUES (?1, ?2)",
687:                 ["world", "50.5"],
688:             )
689:             .unwrap();
690:         }
691: 
692:         let backend =
693:             SqliteDictionaryBackend::from_sqlite(&db_path, "ngrams", "word", "frequency", true)
694:                 .unwrap();
695: 
696:         assert_eq!(backend.len(), 2);
697:         assert!(backend.contains("hello"));
698:         assert!(!backend.contains("foo"));
699:         assert_eq!(backend.frequency("hello"), 100.0);
700:         assert_eq!(backend.frequency("world"), 50.5);
701:     }
702: 
703:     #[test]
704:     fn integer_frequencies() {
705:         let dir = tempdir().unwrap();
706:         let db_path = dir.path().join("test_int.db");
707: 
708:         {
709:             let conn = Connection::open(&db_path).unwrap();
710:             conn.execute(
711:                 "CREATE TABLE words (term TEXT NOT NULL, count INTEGER NOT NULL)",
712:                 [],
713:             )
714:             .unwrap();
715:             conn.execute(
716:                 "INSERT INTO words (term, count) VALUES (?1, ?2)",
717:                 ["apple", "42"],
718:             )
719:             .unwrap();
720:             conn.execute(
721:                 "INSERT INTO words (term, count) VALUES (?1, ?2)",
722:                 ["banana", "17"],
723:             )
724:             .unwrap();
725:         }
726: 
727:         let backend =
728:             SqliteDictionaryBackend::from_sqlite(&db_path, "words", "term", "count", false)
729:                 .unwrap();
730: 
731:         assert_eq!(backend.frequency("apple"), 42.0);
732:         assert_eq!(backend.frequency("banana"), 17.0);
733:     }
734: 
735:     #[test]
736:     fn prefix_query() {
737:         let dir = tempdir().unwrap();
738:         let db_path = dir.path().join("test_pref.db");
739: 
740:         {
741:             let conn = Connection::open(&db_path).unwrap();
742:             conn.execute("CREATE TABLE dict (w TEXT NOT NULL, f REAL NOT NULL)", [])
743:                 .unwrap();
744:             for (w, f) in &[("hello", 10.0), ("help", 5.0), ("world", 1.0)] {
745:                 conn.execute(
746:                     "INSERT INTO dict (w, f) VALUES (?1, ?2)",
747:                     [*w, &f.to_string()],
748:                 )
749:                 .unwrap();
750:             }
751:         }
752: 
753:         let backend =
754:             SqliteDictionaryBackend::from_sqlite(&db_path, "dict", "w", "f", true).unwrap();
755:         let results = backend.query_prefixes(&[DictionaryQuery {
756:             prefix: Some("hel".to_string()),
757:             suffix: None,
758:             min_length: None,
759:             max_length: None,
760:         }]);
761: 
762:         assert_eq!(results.len(), 2);
763:         assert!(results.iter().any(|r| r.word == "hello"));
764:         assert!(results.iter().any(|r| r.word == "help"));
765:     }
766: 
767:     #[test]
768:     fn shared_connection() {
769:         let dir = tempdir().unwrap();
770:         let db_path = dir.path().join("shared.db");
771: 
772:         {
773:             let conn = Connection::open(&db_path).unwrap();
774:             conn.execute("CREATE TABLE words (w TEXT, f REAL)", [])
775:                 .unwrap();
776:             conn.execute("INSERT INTO words VALUES ('hello', 1.0)", [])
777:                 .unwrap();
778:         }
779: 
780:         let shared = SharedSqliteConnection::open(&db_path).unwrap();
781:         let be = SqliteDictionaryBackend::from_shared(shared, "words", "w", "f", true);
782:         assert!(be.contains("hello"));
783:         assert_eq!(be.frequency("hello"), 1.0);
784:     }
785: 
786:     #[test]
787:     fn ngram_unigram_mode() {
788:         let conn = Connection::open(":memory:").unwrap();
789:         conn.execute_batch(
790:             "CREATE TABLE ngrams (context_1 TEXT, context_2 TEXT, next_word TEXT NOT NULL, frequency REAL NOT NULL);
791:              INSERT INTO ngrams VALUES (NULL, NULL, 'der', 1000);
792:              INSERT INTO ngrams VALUES (NULL, NULL, 'die', 800);
793:              INSERT INTO ngrams VALUES (NULL, NULL, 'und', 600);
794:              INSERT INTO ngrams VALUES ('der', NULL, 'stadt', 500);
795:              INSERT INTO ngrams VALUES ('der', NULL, 'fluss', 300);",
796:         )
797:         .unwrap();
798:         let shared = SharedSqliteConnection::new(conn);
799:         let be = SqliteDictionaryBackend::from_ngram_unigrams(
800:             shared,
801:             "ngrams",
802:             &["context_1".to_string(), "context_2".to_string()],
803:             "next_word",
804:             "frequency",
805:             true,
806:         );
807: 
808:         assert_eq!(be.len(), 3);
809:         assert!(be.contains("der"));
810:         assert!(be.contains("die"));
811:         assert!(!be.contains("stadt"));
812:         assert_eq!(be.frequency("der"), 1000.0);
813:         assert_eq!(be.frequency("stadt"), 0.0);
814: 
815:         let results = be.query_prefixes(&[DictionaryQuery {
816:             prefix: Some("d".to_string()),
817:             suffix: None,
818:             min_length: None,
819:             max_length: None,
820:         }]);
821:         assert_eq!(results.len(), 2);
822:         assert!(results.iter().any(|r| r.word == "der"));
823:         assert!(results.iter().any(|r| r.word == "die"));
824:     }
825: 
826:     #[test]
827:     fn readonly_not_writable() {
828:         let dir = tempdir().unwrap();
829:         let db_path = dir.path().join("ro.db");
830: 
831:         {
832:             let conn = Connection::open(&db_path).unwrap();
833:             conn.execute("CREATE TABLE words (w TEXT, f REAL)", [])
834:                 .unwrap();
835:         }
836: 
837:         let be =
838:             SqliteDictionaryBackend::from_sqlite_readonly(&db_path, "words", "w", "f").unwrap();
839:         assert!(!be.is_writable());
840:         let err = be.add_word("test", 1.0, true);
841:         assert!(err.is_err());
842:     }
843: 
844:     #[test]
845:     fn add_word_simple() {
846:         let dir = tempdir().unwrap();
847:         let db_path = dir.path().join("aw.db");
848: 
849:         {
850:             let conn = Connection::open(&db_path).unwrap();
851:             conn.execute("CREATE TABLE words (w TEXT NOT NULL UNIQUE, f REAL)", [])
852:                 .unwrap();
853:         }
854: 
855:         let be = SqliteDictionaryBackend::from_sqlite(&db_path, "words", "w", "f", true).unwrap();
856: 
857:         be.add_word("hello", 10.0, false).unwrap();
858:         assert!(be.contains("hello"));
859:         assert_eq!(be.frequency("hello"), 10.0);
860: 
861:         let err = be.add_word("hello", 20.0, false);
862:         assert!(err.is_err());
863: 
864:         be.add_word("hello", 20.0, true).unwrap();
865:         assert_eq!(be.frequency("hello"), 20.0);
866:     }
867: 
868:     #[test]
869:     fn add_word_ngram_unigram_mode() {
870:         let conn = Connection::open(":memory:").unwrap();
871:         conn.execute_batch(
872:             "CREATE TABLE ngrams (context_1 TEXT, context_2 TEXT, next_word TEXT NOT NULL, frequency REAL NOT NULL, UNIQUE(next_word));
873:              INSERT INTO ngrams VALUES (NULL, NULL, 'hello', 10.0);",
874:         )
875:         .unwrap();
876:         let shared = SharedSqliteConnection::new(conn);
877:         let be = SqliteDictionaryBackend::from_ngram_unigrams(
878:             shared,
879:             "ngrams",
880:             &["context_1".to_string(), "context_2".to_string()],
881:             "next_word",
882:             "frequency",
883:             true,
884:         );
885: 
886:         assert!(be.is_writable());
887:         be.add_word("world", 5.0, false).unwrap();
888:         assert!(be.contains("world"));
889:         assert_eq!(be.frequency("world"), 5.0);
890:         assert!(be.contains("hello"));
891:         let err = be.add_word("hello", 20.0, false);
892:         assert!(err.is_err());
893:         be.add_word("hello", 20.0, true).unwrap();
894:         assert_eq!(be.frequency("hello"), 20.0);
895:     }
896: }

(End of file - total 896 lines)
</content>
```

**Tool: read**

**Input:**
```json
{
  "filePath": "/projects/llm_swapspace/verbisage/src/prediction/sqlite.rs"
}
```

**Output:**
```
<path>/projects/llm_swapspace/verbisage/src/prediction/sqlite.rs</path>
<type>file</type>
<content>
1: use std::path::Path;
2: 
3: use crate::backends::SharedSqliteConnection;
4: use crate::prediction::ngram_backend::NgramBackend;
5: 
6: /// N‑gram data access layer backed by a SQLite table.
7: ///
8: /// The expected table schema is:
9: ///
10: /// ```sql
11: /// CREATE TABLE ngrams (
12: ///     context_1  TEXT,
13: ///     context_2  TEXT,       -- optional, for 3‑grams
14: ///     next_word  TEXT   NOT NULL,
15: ///     frequency  REAL   NOT NULL
16: /// );
17: /// ```
18: ///
19: /// For unigrams, `context_1` is `NULL`.
20: pub struct SqliteNgramBackend {
21:     conn: SharedSqliteConnection,
22:     table_name: String,
23:     context_columns: Vec<String>,
24:     next_word_column: String,
25:     frequency_column: String,
26:     max_order: usize,
27:     unigram_total: u64,
28:     writable: bool,
29: }
30: 
31: impl SqliteNgramBackend {
32:     pub fn new(
33:         conn: SharedSqliteConnection,
34:         table_name: &str,
35:         context_columns: &[String],
36:         next_word_column: &str,
37:         frequency_column: &str,
38:         max_order: usize,
39:         writable: bool,
40:     ) -> Self {
41:         let unigram_total =
42:             Self::query_unigram_total(&conn, table_name, context_columns, frequency_column);
43:         Self {
44:             conn,
45:             table_name: table_name.to_string(),
46:             context_columns: context_columns.to_vec(),
47:             next_word_column: next_word_column.to_string(),
48:             frequency_column: frequency_column.to_string(),
49:             max_order,
50:             unigram_total,
51:             writable,
52:         }
53:     }
54: 
55:     pub fn from_path(
56:         path: &Path,
57:         table_name: &str,
58:         context_columns: &[String],
59:         next_word_column: &str,
60:         frequency_column: &str,
61:         max_order: usize,
62:         writable: bool,
63:     ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
64:         let conn = SharedSqliteConnection::open(path)?;
65:         Ok(Self::new(
66:             conn,
67:             table_name,
68:             context_columns,
69:             next_word_column,
70:             frequency_column,
71:             max_order,
72:             writable,
73:         ))
74:     }
75: 
76:     fn query_unigram_total(
77:         conn: &SharedSqliteConnection,
78:         table_name: &str,
79:         context_columns: &[String],
80:         frequency_column: &str,
81:     ) -> u64 {
82:         let null_conditions: Vec<String> = context_columns
83:             .iter()
84:             .map(|c| format!("{} IS NULL", c))
85:             .collect();
86:         let sql = format!(
87:             "SELECT COALESCE(SUM({}), 0) FROM {} WHERE {}",
88:             frequency_column,
89:             table_name,
90:             null_conditions.join(" AND ")
91:         );
92:         let conn = conn.lock();
93:         if let Ok(mut stmt) = conn.prepare(&sql) {
94:             if let Ok(mut rows) = stmt.query(()) {
95:                 if let Ok(row) = rows.next() {
96:                     if let Ok(val) = row.unwrap().get::<_, f64>(0) {
97:                         return val as u64;
98:                     }
99:                 }
100:             }
101:         }
102:         0
103:     }
104: }
105: 
106: impl NgramBackend for SqliteNgramBackend {
107:     fn max_order(&self) -> usize {
108:         self.max_order
109:     }
110: 
111:     fn unigram_total(&self) -> u64 {
112:         self.unigram_total
113:     }
114: 
115:     fn ngram_count(&self, ngram: &[&str]) -> u64 {
116:         let order = ngram.len();
117:         if order == 0 {
118:             return 0;
119:         }
120:         let context: Vec<&str> = ngram[..order - 1].iter().copied().collect();
121:         let next_word = ngram[order - 1];
122: 
123:         if order == 1 {
124:             let sql = format!(
125:                 "SELECT COALESCE(SUM({}), 0) FROM {} WHERE {} = ?1",
126:                 self.frequency_column, self.table_name, self.next_word_column
127:             );
128:             let conn = self.conn.lock();
129:             if let Ok(mut stmt) = conn.prepare(&sql) {
130:                 if let Ok(mut rows) = stmt.query([next_word]) {
131:                     if let Ok(row) = rows.next() {
132:                         if let Ok(val) = row.unwrap().get::<_, f64>(0) {
133:                             return val as u64;
134:                         }
135:                     }
136:                 }
137:             }
138:             return 0;
139:         }
140: 
141:         let mut conditions = Vec::new();
142:         let mut params: Vec<&dyn rusqlite::types::ToSql> = Vec::new();
143:         for (i, col) in self.context_columns.iter().enumerate() {
144:             if let Some(ctx) = context.get(i) {
145:                 conditions.push(format!("{} = ?{}", col, i + 1));
146:                 params.push(ctx as &dyn rusqlite::types::ToSql);
147:             }
148:         }
149:         conditions.push(format!("{} = ?{}", self.next_word_column, params.len() + 1));
150:         params.push(&next_word as &dyn rusqlite::types::ToSql);
151: 
152:         let sql = format!(
153:             "SELECT COALESCE(SUM({}), 0) FROM {} WHERE {}",
154:             self.frequency_column,
155:             self.table_name,
156:             conditions.join(" AND ")
157:         );
158:         let conn = self.conn.lock();
159:         if let Ok(mut stmt) = conn.prepare(&sql) {
160:             if let Ok(mut rows) = stmt.query(params.as_slice()) {
161:                 if let Ok(row) = rows.next() {
162:                     if let Ok(val) = row.unwrap().get::<_, f64>(0) {
163:                         return val as u64;
164:                     }
165:                 }
166:             }
167:         }
168:         0
169:     }
170: 
171:     fn candidates(&self, context: &[&str], max_candidates: usize) -> Vec<(String, u64)> {
172:         let order = context.len() + 1;
173: 
174:         if order == 1 {
175:             let sql = format!(
176:                 "SELECT {}, {} FROM {} WHERE {} IS NULL ORDER BY {} DESC LIMIT {}",
177:                 self.next_word_column,
178:                 self.frequency_column,
179:                 self.table_name,
180:                 self.context_columns
181:                     .first()
182:                     .unwrap_or(&"context_1".to_string()),
183:                 self.frequency_column,
184:                 max_candidates
185:             );
186:             let conn = self.conn.lock();
187:             if let Ok(mut stmt) = conn.prepare(&sql) {
188:                 if let Ok(rows) = stmt.query_map((), |row| {
189:                     let word: String = row.get(0)?;
190:                     let freq: f64 = row.get(1)?;
191:                     Ok((word, freq as u64))
192:                 }) {
193:                     return rows.filter_map(|r| r.ok()).collect();
194:                 }
195:             }
196:             return Vec::new();
197:         }
198: 
199:         let mut conditions = Vec::new();
200:         let mut params: Vec<&dyn rusqlite::types::ToSql> = Vec::new();
201:         for (i, col) in self.context_columns.iter().enumerate() {
202:             if let Some(ctx) = context.get(i) {
203:                 conditions.push(format!("{} = ?{}", col, i + 1));
204:                 params.push(ctx as &dyn rusqlite::types::ToSql);
205:             }
206:         }
207: 
208:         let sql = format!(
209:             "SELECT {}, {} FROM {} WHERE {} ORDER BY {} DESC LIMIT {}",
210:             self.next_word_column,
211:             self.frequency_column,
212:             self.table_name,
213:             if conditions.is_empty() {
214:                 "1=1".to_string()
215:             } else {
216:                 conditions.join(" AND ")
217:             },
218:             self.frequency_column,
219:             max_candidates
220:         );
221:         let conn = self.conn.lock();
222:         if let Ok(mut stmt) = conn.prepare(&sql) {
223:             if let Ok(rows) = stmt.query_map(params.as_slice(), |row| {
224:                 let word: String = row.get(0)?;
225:                 let freq: f64 = match row.get::<_, f64>(1) {
226:                     Ok(f) => f,
227:                     Err(_) => {
228:                         let int_freq: i64 = row.get(1)?;
229:                         int_freq as f64
230:                     }
231:                 };
232:                 Ok((word, freq as u64))
233:             }) {
234:                 return rows.filter_map(|r| r.ok()).collect();
235:             }
236:         }
237:         Vec::new()
238:     }
239: 
240:     fn is_writable(&self) -> bool {
241:         self.writable
242:     }
243: 
244:     fn increase_ngram_frequency(
245:         &self,
246:         ngram: &[&str],
247:         delta: f64,
248:         save_unknown: bool,
249:     ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
250:         if !self.writable {
251:             return Err("backend is not writable".into());
252:         }
253:         if ngram.is_empty() {
254:             return Err("ngram must have at least one element".into());
255:         }
256:         if delta < 0.0 {
257:             return Err("delta must be non-negative".into());
258:         }
259: 
260:         let order = ngram.len();
261:         let context: Vec<&str> = ngram[..order - 1].iter().copied().collect();
262:         let next_word = ngram[order - 1];
263: 
264:         let conn = self.conn.lock();
265:         let rows_changed = if order == 1 {
266:             self.update_unigram(&conn, next_word, delta, save_unknown)?
267:         } else {
268:             self.update_context_ngram(&conn, &context, next_word, delta, save_unknown)?
269:         };
270: 
271:         if !save_unknown && rows_changed == 0 {
272:             return Err(format!("ngram {:?} not found in backend", ngram).into());
273:         }
274: 
275:         Ok(())
276:     }
277: }
278: 
279: impl SqliteNgramBackend {
280:     fn update_unigram(
281:         &self,
282:         conn: &rusqlite::Connection,
283:         next_word: &str,
284:         delta: f64,
285:         save_unknown: bool,
286:     ) -> Result<usize, Box<dyn std::error::Error + Send + Sync>> {
287:         let null_conditions: Vec<String> = self
288:             .context_columns
289:             .iter()
290:             .map(|c| format!("{} IS NULL", c))
291:             .collect();
292:         let where_clause = format!(
293:             "{} AND {} = ?1",
294:             null_conditions.join(" AND "),
295:             self.next_word_column
296:         );
297: 
298:         let update_sql = format!(
299:             "UPDATE {} SET {} = {} + ?2 WHERE {}",
300:             self.table_name, self.frequency_column, self.frequency_column, where_clause
301:         );
302:         let rows = conn.execute(
303:             &update_sql,
304:             [&next_word as &dyn rusqlite::types::ToSql, &delta],
305:         )?;
306: 
307:         if rows == 0 && save_unknown {
308:             let context_placeholders: Vec<String> = self
309:                 .context_columns
310:                 .iter()
311:                 .map(|_| "NULL".to_string())
312:                 .collect();
313:             let columns = format!(
314:                 "({}, {}, {})",
315:                 self.context_columns.join(", "),
316:                 self.next_word_column,
317:                 self.frequency_column
318:             );
319:             let values = format!("({}, ?, ?)", context_placeholders.join(", "));
320:             let insert_sql = format!(
321:                 "INSERT INTO {} {} VALUES {}",
322:                 self.table_name, columns, values
323:             );
324:             conn.execute(
325:                 &insert_sql,
326:                 [&next_word as &dyn rusqlite::types::ToSql, &delta],
327:             )?;
328:             Ok(1)
329:         } else {
330:             Ok(rows)
331:         }
332:     }
333: 
334:     fn update_context_ngram(
335:         &self,
336:         conn: &rusqlite::Connection,
337:         context: &[&str],
338:         next_word: &str,
339:         delta: f64,
340:         save_unknown: bool,
341:     ) -> Result<usize, Box<dyn std::error::Error + Send + Sync>> {
342:         let mut conditions = Vec::new();
343:         let mut params: Vec<&dyn rusqlite::types::ToSql> = Vec::new();
344:         for (i, col) in self.context_columns.iter().enumerate() {
345:             if let Some(ctx) = context.get(i) {
346:                 conditions.push(format!("{} = ?{}", col, i + 1));
347:                 params.push(ctx as &dyn rusqlite::types::ToSql);
348:             }
349:         }
350:         conditions.push(format!("{} = ?{}", self.next_word_column, params.len() + 1));
351:         params.push(&next_word as &dyn rusqlite::types::ToSql);
352: 
353:         let where_clause = conditions.join(" AND ");
354:         let delta_param_idx = params.len() + 1;
355:         let update_sql = format!(
356:             "UPDATE {} SET {} = {} + ?{} WHERE {}",
357:             self.table_name,
358:             self.frequency_column,
359:             self.frequency_column,
360:             delta_param_idx,
361:             where_clause
362:         );
363: 
364:         let mut all_params = params.to_vec();
365:         all_params.push(&delta as &dyn rusqlite::types::ToSql);
366:         let rows = conn.execute(&update_sql, all_params.as_slice())?;
367: 
368:         if rows == 0 && save_unknown {
369:             let mut insert_params: Vec<&dyn rusqlite::types::ToSql> = Vec::new();
370:             for ctx in context {
371:                 insert_params.push(ctx as &dyn rusqlite::types::ToSql);
372:             }
373:             insert_params.push(&next_word as &dyn rusqlite::types::ToSql);
374:             insert_params.push(&delta as &dyn rusqlite::types::ToSql);
375: 
376:             let placeholders: Vec<String> = (1..=insert_params.len())
377:                 .map(|i| format!("?{}", i))
378:                 .collect();
379:             let columns = format!(
380:                 "({}, {}, {})",
381:                 self.context_columns.join(", "),
382:                 self.next_word_column,
383:                 self.frequency_column
384:             );
385:             let insert_sql = format!(
386:                 "INSERT INTO {} {} VALUES ({})",
387:                 self.table_name,
388:                 columns,
389:                 placeholders.join(", ")
390:             );
391:             conn.execute(&insert_sql, insert_params.as_slice())?;
392:             Ok(1)
393:         } else {
394:             Ok(rows)
395:         }
396:     }
397: }
398: 
399: #[cfg(test)]
400: mod tests {
401:     use super::*;
402:     use rusqlite::Connection;
403: 
404:     #[test]
405:     fn unigram_total() {
406:         let conn = Connection::open(":memory:").unwrap();
407:         conn.execute_batch(
408:             "CREATE TABLE ngrams (context_1 TEXT, context_2 TEXT, next_word TEXT NOT NULL, frequency REAL NOT NULL);
409:              INSERT INTO ngrams VALUES (NULL, NULL, 'der', 1000);
410:              INSERT INTO ngrams VALUES (NULL, NULL, 'die', 800);
411:              INSERT INTO ngrams VALUES (NULL, NULL, 'und', 600);
412:              INSERT INTO ngrams VALUES ('der', NULL, 'stadt', 500);",
413:         )
414:         .unwrap();
415:         let shared = SharedSqliteConnection::new(conn);
416:         let backend = SqliteNgramBackend::new(
417:             shared,
418:             "ngrams",
419:             &["context_1".to_string(), "context_2".to_string()],
420:             "next_word",
421:             "frequency",
422:             3,
423:             false,
424:         );
425:         assert_eq!(backend.unigram_total(), 2400);
426:     }
427: 
428:     #[test]
429:     fn ngram_count_unigram() {
430:         let conn = Connection::open(":memory:").unwrap();
431:         conn.execute_batch(
432:             "CREATE TABLE ngrams (context_1 TEXT, next_word TEXT NOT NULL, frequency REAL NOT NULL);
433:              INSERT INTO ngrams VALUES (NULL, 'der', 1000);
434:              INSERT INTO ngrams VALUES ('how', 'are', 50);",
435:         )
436:         .unwrap();
437:         let shared = SharedSqliteConnection::new(conn);
438:         let backend = SqliteNgramBackend::new(
439:             shared,
440:             "ngrams",
441:             &["context_1".to_string()],
442:             "next_word",
443:             "frequency",
444:             2,
445:             false,
446:         );
447:         assert_eq!(backend.ngram_count(&["der"]), 1000);
448:         assert_eq!(backend.ngram_count(&["foo"]), 0);
449:     }
450: 
451:     #[test]
452:     fn ngram_count_bigram() {
453:         let conn = Connection::open(":memory:").unwrap();
454:         conn.execute_batch(
455:             "CREATE TABLE ngrams (context_1 TEXT, next_word TEXT NOT NULL, frequency REAL NOT NULL);
456:              INSERT INTO ngrams VALUES ('how', 'are', 50);
457:              INSERT INTO ngrams VALUES ('how', 'is', 30);",
458:         )
459:         .unwrap();
460:         let shared = SharedSqliteConnection::new(conn);
461:         let backend = SqliteNgramBackend::new(
462:             shared,
463:             "ngrams",
464:             &["context_1".to_string()],
465:             "next_word",
466:             "frequency",
467:             2,
468:             false,
469:         );
470:         assert_eq!(backend.ngram_count(&["how", "are"]), 50);
471:         assert_eq!(backend.ngram_count(&["how", "you"]), 0);
472:     }
473: 
474:     #[test]
475:     fn candidates_unigram() {
476:         let conn = Connection::open(":memory:").unwrap();
477:         conn.execute_batch(
478:             "CREATE TABLE ngrams (context_1 TEXT, next_word TEXT NOT NULL, frequency REAL NOT NULL);
479:              INSERT INTO ngrams VALUES (NULL, 'der', 1000);
480:              INSERT INTO ngrams VALUES (NULL, 'die', 800);
481:              INSERT INTO ngrams VALUES (NULL, 'und', 600);",
482:         )
483:         .unwrap();
484:         let shared = SharedSqliteConnection::new(conn);
485:         let backend = SqliteNgramBackend::new(
486:             shared,
487:             "ngrams",
488:             &["context_1".to_string()],
489:             "next_word",
490:             "frequency",
491:             2,
492:             false,
493:         );
494:         let results = backend.candidates(&[], 2);
495:         assert_eq!(results.len(), 2);
496:         assert_eq!(results[0], ("der".to_string(), 1000));
497:         assert_eq!(results[1], ("die".to_string(), 800));
498:     }
499: 
500:     #[test]
501:     fn candidates_bigram() {
502:         let conn = Connection::open(":memory:").unwrap();
503:         conn.execute_batch(
504:             "CREATE TABLE bigrams (prev TEXT, next TEXT, freq REAL);
505:              INSERT INTO bigrams VALUES ('how', 'are', 50);
506:              INSERT INTO bigrams VALUES ('how', 'is', 30);
507:              INSERT INTO bigrams VALUES ('how', 'you', 20);",
508:         )
509:         .unwrap();
510:         let shared = SharedSqliteConnection::new(conn);
511:         let backend = SqliteNgramBackend::new(
512:             shared,
513:             "bigrams",
514:             &["prev".to_string()],
515:             "next",
516:             "freq",
517:             2,
518:             false,
519:         );
520:         let results = backend.candidates(&["how"], 2);
521:         assert_eq!(results.len(), 2);
522:         assert_eq!(results[0], ("are".to_string(), 50));
523:         assert_eq!(results[1], ("is".to_string(), 30));
524:     }
525: 
526:     #[test]
527:     fn increase_ngram_freq_unigram_save_unknown() {
528:         let conn = Connection::open(":memory:").unwrap();
529:         conn.execute_batch(
530:             "CREATE TABLE ngrams (context_1 TEXT, next_word TEXT NOT NULL, frequency REAL NOT NULL);
531:              INSERT INTO ngrams VALUES (NULL, 'der', 1000);",
532:         )
533:         .unwrap();
534:         let shared = SharedSqliteConnection::new(conn);
535:         let backend = SqliteNgramBackend::new(
536:             shared,
537:             "ngrams",
538:             &["context_1".to_string()],
539:             "next_word",
540:             "frequency",
541:             2,
542:             true,
543:         );
544:         assert!(backend.is_writable());
545:         backend
546:             .increase_ngram_frequency(&["der"], 5.0, false)
547:             .unwrap();
548:         assert_eq!(backend.ngram_count(&["der"]), 1005);
549: 
550:         backend
551:             .increase_ngram_frequency(&["newword"], 10.0, true)
552:             .unwrap();
553:         assert_eq!(backend.ngram_count(&["newword"]), 10);
554:     }
555: 
556:     #[test]
557:     fn increase_ngram_freq_bigram_save_unknown() {
558:         let conn = Connection::open(":memory:").unwrap();
559:         conn.execute_batch(
560:             "CREATE TABLE ngrams (context_1 TEXT, next_word TEXT NOT NULL, frequency REAL NOT NULL);
561:              INSERT INTO ngrams VALUES ('how', 'are', 50);",
562:         )
563:         .unwrap();
564:         let shared = SharedSqliteConnection::new(conn);
565:         let backend = SqliteNgramBackend::new(
566:             shared,
567:             "ngrams",
568:             &["context_1".to_string()],
569:             "next_word",
570:             "frequency",
571:             2,
572:             true,
573:         );
574:         backend
575:             .increase_ngram_frequency(&["how", "are"], 3.0, false)
576:             .unwrap();
577:         assert_eq!(backend.ngram_count(&["how", "are"]), 53);
578: 
579:         backend
580:             .increase_ngram_frequency(&["how", "is"], 20.0, true)
581:             .unwrap();
582:         assert_eq!(backend.ngram_count(&["how", "is"]), 20);
583:     }
584: 
585:     #[test]
586:     fn increase_ngram_freq_readonly_rejected() {
587:         let conn = Connection::open(":memory:").unwrap();
588:         conn.execute_batch(
589:             "CREATE TABLE ngrams (context_1 TEXT, next_word TEXT NOT NULL, frequency REAL NOT NULL);",
590:         )
591:         .unwrap();
592:         let shared = SharedSqliteConnection::new(conn);
593:         let backend = SqliteNgramBackend::new(
594:             shared,
595:             "ngrams",
596:             &["context_1".to_string()],
597:             "next_word",
598:             "frequency",
599:             2,
600:             false,
601:         );
602:         assert!(!backend.is_writable());
603:         let err = backend.increase_ngram_frequency(&["test"], 1.0, true);
604:         assert!(err.is_err());
605:     }
606: 
607:     #[test]
608:     fn increase_ngram_freq_save_unknown_false_rejected() {
609:         let conn = Connection::open(":memory:").unwrap();
610:         conn.execute_batch(
611:             "CREATE TABLE ngrams (context_1 TEXT, next_word TEXT NOT NULL, frequency REAL NOT NULL);",
612:         )
613:         .unwrap();
614:         let shared = SharedSqliteConnection::new(conn);
615:         let backend = SqliteNgramBackend::new(
616:             shared,
617:             "ngrams",
618:             &["context_1".to_string()],
619:             "next_word",
620:             "frequency",
621:             2,
622:             true,
623:         );
624:         let err = backend.increase_ngram_frequency(&["nonexistent"], 1.0, false);
625:         assert!(err.is_err());
626:     }
627: 
628:     #[test]
629:     fn increase_ngram_freq_via_predictor_trait() {
630:         use crate::prediction::Predictor;
631:         let conn = Connection::open(":memory:").unwrap();
632:         conn.execute_batch(
633:             "CREATE TABLE ngrams (context_1 TEXT, next_word TEXT NOT NULL, frequency REAL NOT NULL);
634:              INSERT INTO ngrams VALUES (NULL, 'hello', 10.0);",
635:         )
636:         .unwrap();
637:         let shared = SharedSqliteConnection::new(conn);
638:         let backend = SqliteNgramBackend::new(
639:             shared,
640:             "ngrams",
641:             &["context_1".to_string()],
642:             "next_word",
643:             "frequency",
644:             2,
645:             true,
646:         );
647:         let predictor = crate::prediction::smoothed::SmoothedPredictor::new(Box::new(backend))
648:             .with_deltas(vec![0.4, 0.4, 0.2]);
649: 
650:         assert_eq!(predictor.ngram_count(&["hello"]), 10);
651:         predictor
652:             .increase_ngram_frequency(&["hello"], 5.0, false)
653:             .unwrap();
654:         assert_eq!(predictor.ngram_count(&["hello"]), 15);
655:     }
656: }

(End of file - total 656 lines)
</content>
```

**Tool: read**

**Input:**
```json
{
  "filePath": "/projects/llm_swapspace/verbisage/src/backends/build.rs"
}
```

**Output:**
```
<path>/projects/llm_swapspace/verbisage/src/backends/build.rs</path>
<type>file</type>
<content>
1: use std::path::PathBuf;
2: use std::sync::Arc;
3: 
4: use crate::dictionary::paths::{LanguagePaths, expand_tilde};
5: use crate::dictionary::{DictionaryBackend, FileDictionaryBackend};
6: use crate::prediction::{Predictor, smoothed::SmoothedPredictor};
7: use crate::spellcheck::{DictionarySpellChecker, SpellChecker};
8: 
9: #[cfg(feature = "sqlite")]
10: use crate::backends::SharedSqliteConnection;
11: #[cfg(feature = "sqlite")]
12: use crate::dictionary::SqliteDictionaryBackend;
13: #[cfg(feature = "sqlite")]
14: use crate::prediction::sqlite::SqliteNgramBackend;
15: #[cfg(feature = "sqlite")]
16: use crate::spellcheck::SqliteSpellChecker;
17: 
18: #[cfg(feature = "hunspell")]
19: use crate::dictionary::HunspellDictionaryBackend;
20: 
21: #[cfg(feature = "hunspell")]
22: use crate::spellcheck::HunspellSpellChecker;
23: 
24: use super::chain::SegmentRole;
25: use super::merged::{MergedDictionary, MergedPredictor};
26: use super::{BackendType, Capability, ResolvedBackendDef};
27: 
28: /// Normalize a directory path string: expand tilde and ensure no trailing slash.
29: /// This ensures consistent behavior regardless of whether the user includes
30: /// a trailing slash in their config.
31: fn normalize_dir(dir: &str) -> PathBuf {
32:     let expanded = expand_tilde(dir);
33:     let path_str = expanded.to_string_lossy();
34:     // Strip trailing slashes (but keep root "/")
35:     let trimmed = path_str.trim_end_matches('/');
36:     PathBuf::from(if trimmed.is_empty() { "/" } else { trimmed })
37: }
38: 
39: // ---------------------------------------------------------------------------
40: // Per-backend build
41: // ---------------------------------------------------------------------------
42: 
43: /// Build a concrete set of backend objects for one resolved backend definition.
44: pub fn build_backend(
45:     def: &ResolvedBackendDef,
46:     lang: &str,
47:     lp: &LanguagePaths,
48: ) -> (
49:     Box<dyn DictionaryBackend>,
50:     Option<Box<dyn SpellChecker>>,
51:     Option<Box<dyn Predictor>>,
52: ) {
53:     match def.backend_type {
54:         BackendType::File => build_file(def, lang, lp),
55:         BackendType::Sqlite => build_sqlite(def, lang, lp),
56:         BackendType::Marisa => build_marisa(def, lang, lp),
57:         BackendType::Hunspell => build_hunspell(def, lang, lp),
58:     }
59: }
60: 
61: // ---------------------------------------------------------------------------
62: // File backend
63: // ---------------------------------------------------------------------------
64: 
65: fn build_file(
66:     def: &ResolvedBackendDef,
67:     lang: &str,
68:     lp: &LanguagePaths,
69: ) -> (
70:     Box<dyn DictionaryBackend>,
71:     Option<Box<dyn SpellChecker>>,
72:     Option<Box<dyn Predictor>>,
73: ) {
74:     let files = resolve_files(def, lang, lp);
75: 
76:     if files.is_empty() {
77:         eprintln!("warning: no file dictionary found for '{}'", lang);
78:     }
79: 
80:     let user_dir = lp.user_dir.as_os_str().to_str().unwrap_or("").to_string();
81:     let user_files: Vec<_> = if user_dir.is_empty() {
82:         Vec::new()
83:     } else {
84:         files
85:             .iter()
86:             .filter(|f| {
87:                 f.to_str()
88:                     .map(|p| p.starts_with(&user_dir))
89:                     .unwrap_or(false)
90:             })
91:             .collect()
92:     };
93:     let writable = user_files.len() == files.len() && !files.is_empty();
94: 
95:     let dict: FileDictionaryBackend = if files.is_empty() {
96:         FileDictionaryBackend::new()
97:     } else {
98:         for f in &files {
99:             eprintln!("info: loaded file dictionary: {}", f.display());
100:         }
101:         if let Some(delim) = &def.delimiter {
102:             if let Some(word_index) = def.word_index {
103:                 // Delimited mode (CSV, TSV, etc.) with explicit column indexes.
104:                 let delim_byte = delim.as_bytes().first().copied().unwrap_or(b',');
105:                 let merged = FileDictionaryBackend::new();
106:                 for f in &files {
107:                     match FileDictionaryBackend::from_delimited_file(
108:                         f,
109:                         delim_byte,
110:                         def.has_header,
111:                         word_index,
112:                         def.freq_index,
113:                         writable,
114:                     ) {
115:                         Ok(other) => merged.merge(&other),
116:                         Err(e) => eprintln!("warning: failed to load '{}': {}", f.display(), e),
117:                     }
118:                 }
119:                 merged
120:             } else {
121:                 // Delimiter set but no word_index — treat as flat word-per-line.
122:                 eprintln!("warning: delimiter set but no word_index; falling back to flat mode");
123:                 FileDictionaryBackend::from_multiple_files(&files, writable).unwrap_or_else(|e| {
124:                     eprintln!("warning: failed to load file backend: {}", e);
125:                     FileDictionaryBackend::new()
126:                 })
127:             }
128:         } else {
129:             // Flat / freq mode — auto-detect whitespace-separated or line-separated
130:             FileDictionaryBackend::from_multiple_files(&files, writable).unwrap_or_else(|e| {
131:                 eprintln!("warning: failed to load file backend: {}", e);
132:                 FileDictionaryBackend::new()
133:             })
134:         }
135:     };
136: 
137:     let dict_arc: Arc<FileDictionaryBackend> = Arc::new(dict);
138:     let sc: Box<dyn SpellChecker> = Box::new(DictionarySpellChecker::new(dict_arc.clone()));
139:     let dict_box: Box<dyn DictionaryBackend> = Box::new(dict_arc);
140:     (dict_box, Some(sc), None)
141: }
142: 
143: // ---------------------------------------------------------------------------
144: // SQLite backend
145: // ---------------------------------------------------------------------------
146: 
147: #[cfg(feature = "sqlite")]
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
348: 
349:     let trie_path = trie_candidates.iter().cloned().find(|p| p.exists());
350:     let trie_path = match trie_path {
351:         Some(p) => p,
352:         None => {
353:             eprintln!(
354:                 "warning: no marisa ngram trie found for '{}' (tried: {})",
355:                 lang,
356:                 trie_candidates
357:                     .iter()
358:                     .map(|p| p.display().to_string())
359:                     .collect::<Vec<_>>()
360:                     .join(", ")
361:             );
362:             return (Box::new(FileDictionaryBackend::new()), None, None);
363:         }
364:     };
365: 
366:     let counts_path = counts_candidates.iter().cloned().find(|p| p.exists());
367:     let counts_path = match counts_path {
368:         Some(p) => p,
369:         None => {
370:             eprintln!(
371:                 "warning: no marisa ngram counts file found for '{}' (tried: {})",
372:                 lang,
373:                 counts_candidates
374:                     .iter()
375:                     .map(|p| p.display().to_string())
376:                     .collect::<Vec<_>>()
377:                     .join(", ")
378:             );
379:             return (Box::new(FileDictionaryBackend::new()), None, None);
380:         }
381:     };
382: 
383:     let backend =
384:         match crate::prediction::marisa::MarisaNgramBackend::from_files(&trie_path, &counts_path) {
385:             Ok(b) => {
386:                 eprintln!(
387:                     "info: loaded marisa ngram backend: {} + {}",
388:                     trie_path.display(),
389:                     counts_path.display()
390:                 );
391:                 Arc::new(b)
392:             }
393:             Err(e) => {
394:                 eprintln!(
395:                     "warning: failed to load marisa ngram backend '{}': {}",
396:                     trie_path.display(),
397:                     e
398:                 );
399:                 return (Box::new(FileDictionaryBackend::new()), None, None);
400:             }
401:         };
402: 
403:     let dict: Box<dyn DictionaryBackend> = Box::new(backend.clone());
404: 
405:     let spellchecker = {
406:         let checker = crate::spellcheck::DictionarySpellChecker::new(backend.clone());
407:         Some(Box::new(checker) as Box<dyn SpellChecker>)
408:     };
409: 
410:     let predictor = if def.capabilities.contains(&Capability::Ngrams) {
411:         let ngram_backend: Box<dyn crate::prediction::ngram_backend::NgramBackend> =
412:             Box::new(backend);
413:         Some(
414:             Box::new(crate::prediction::smoothed::SmoothedPredictor::new(
415:                 ngram_backend,
416:             )) as Box<dyn crate::prediction::Predictor>,
417:         )
418:     } else {
419:         None
420:     };
421: 
422:     (dict, spellchecker, predictor)
423: }
424: 
425: #[cfg(not(feature = "marisa"))]
426: fn build_marisa(
427:     _def: &ResolvedBackendDef,
428:     _lang: &str,
429:     _lp: &LanguagePaths,
430: ) -> (
431:     Box<dyn DictionaryBackend>,
432:     Option<Box<dyn SpellChecker>>,
433:     Option<Box<dyn Predictor>>,
434: ) {
435:     let _ = _def;
436:     let _ = _lang;
437:     let _ = _lp;
438:     eprintln!("warning: marisa feature not enabled");
439:     (Box::new(FileDictionaryBackend::new()), None, None)
440: }
441: 
442: // ---------------------------------------------------------------------------
443: // Hunspell backend
444: // ---------------------------------------------------------------------------
445: 
446: #[cfg(feature = "hunspell")]
447: fn build_hunspell(
448:     def: &ResolvedBackendDef,
449:     lang: &str,
450:     _lp: &LanguagePaths,
451: ) -> (
452:     Box<dyn DictionaryBackend>,
453:     Option<Box<dyn SpellChecker>>,
454:     Option<Box<dyn Predictor>>,
455: ) {
456:     // Determine the .dic and .aff file paths.
457:     let (aff_path, dic_path) = find_hunspell_files(def, lang);
458: 
459:     let dict = match &dic_path {
460:         Some(path) => match HunspellDictionaryBackend::from_dic_file(path) {
461:             Ok(d) => {
462:                 eprintln!("info: loaded hunspell dictionary: {}", path.display());
463:                 Box::new(d) as Box<dyn DictionaryBackend>
464:             }
465:             Err(e) => {
466:                 eprintln!(
467:                     "warning: failed to load hunspell .dic '{}': {}",
468:                     path.display(),
469:                     e
470:                 );
471:                 Box::new(FileDictionaryBackend::new()) as Box<dyn DictionaryBackend>
472:             }
473:         },
474:         None => Box::new(FileDictionaryBackend::new()) as Box<dyn DictionaryBackend>,
475:     };
476: 
477:     let sc = match (aff_path, dic_path) {
478:         (Some(aff), Some(dic)) => match HunspellSpellChecker::from_files(&aff, &dic) {
479:             Ok(c) => {
480:                 eprintln!(
481:                     "info: loaded hunspell spellchecker: {} + {}",
482:                     aff.display(),
483:                     dic.display()
484:                 );
485:                 Some(Box::new(c) as Box<dyn SpellChecker>)
486:             }
487:             Err(e) => {
488:                 eprintln!("warning: failed to load hunspell from files: {}", e);
489:                 None
490:             }
491:         },
492:         _ => match HunspellSpellChecker::from_tag(lang) {
493:             Ok(c) => {
494:                 eprintln!("info: loaded hunspell spellchecker for tag '{}'", lang);
495:                 Some(Box::new(c) as Box<dyn SpellChecker>)
496:             }
497:             Err(e) => {
498:                 eprintln!(
499:                     "warning: hunspell dictionary not found for '{}': {}",
500:                     lang, e
501:                 );
502:                 None
503:             }
504:         },
505:     };
506: 
507:     (dict, sc, None)
508: }
509: 
510: /// Search for Hunspell `.aff` and `.dic` files, preferring explicit paths
511: /// set on the backend def, then falling back to system directories.
512: #[cfg(feature = "hunspell")]
513: fn find_hunspell_files(
514:     _def: &ResolvedBackendDef,
515:     lang: &str,
516: ) -> (Option<PathBuf>, Option<PathBuf>) {
517:     // Check explicit path — treat path as base name (without extension).
518:     // For now, always fall back to system search.
519:     let dirs = [
520:         "/usr/share/hunspell",
521:         "/usr/share/myspell",
522:         "/usr/share/myspell/dicts",
523:     ];
524: 
525:     for dir in &dirs {
526:         let aff = PathBuf::from(dir).join(format!("{}.aff", lang));
527:         let dic = PathBuf::from(dir).join(format!("{}.dic", lang));
528:         if aff.exists() && dic.exists() {
529:             return (Some(aff), Some(dic));
530:         }
531:     }
532: 
533:     (None, None)
534: }
535: 
536: #[cfg(not(feature = "hunspell"))]
537: fn build_hunspell(
538:     _def: &ResolvedBackendDef,
539:     _lang: &str,
540:     _lp: &LanguagePaths,
541: ) -> (
542:     Box<dyn DictionaryBackend>,
543:     Option<Box<dyn SpellChecker>>,
544:     Option<Box<dyn Predictor>>,
545: ) {
546:     eprintln!("warning: hunspell feature not enabled");
547:     (Box::new(FileDictionaryBackend::new()), None, None)
548: }
549: 
550: // ---------------------------------------------------------------------------
551: // Marisa ngram path resolution
552: // ---------------------------------------------------------------------------
553: 
554: #[cfg(feature = "marisa")]
555: fn resolve_marisa_ngram_trie_files(
556:     def: &ResolvedBackendDef,
557:     lang: &str,
558:     lp: &LanguagePaths,
559: ) -> Vec<PathBuf> {
560:     let mut results = Vec::new();
561: 
562:     if let Some(p) = &def.ngram_path {
563:         let expanded = p.replace("{lang}", lang);
564:         let path = expand_tilde(&expanded);
565:         if !results.contains(&path) {
566:             results.push(path);
567:         }
568:     }
569: 
570:     if let Some(ref dict_path) = def.path {
571:         let expanded = dict_path.replace("{lang}", lang);
572:         let base = PathBuf::from(expand_tilde(&expanded));
573:         let dir = if base.is_dir() {
574:             base
575:         } else if let Some(parent) = base.parent() {
576:             parent.to_path_buf()
577:         } else {
578:             PathBuf::new()
579:         };
580:         if !dir.as_os_str().is_empty() {
581:             let candidate = dir.join("ngrams.trie");
582:             if !results.contains(&candidate) {
583:                 results.push(candidate);
584:             }
585:         }
586:     }
587: 
588:     for p in lp.resolve_all_marisa_ngram_trie_files() {
589:         if !results.contains(&p) {
590:             results.push(p);
591:         }
592:     }
593: 
594:     results
595: }
596: 
597: #[cfg(feature = "marisa")]
598: fn resolve_marisa_ngram_counts_files(
599:     def: &ResolvedBackendDef,
600:     lang: &str,
601:     lp: &LanguagePaths,
602: ) -> Vec<PathBuf> {
603:     let mut results = Vec::new();
604: 
605:     if let Some(p) = &def.ngram_path {
606:         let expanded = p.replace("{lang}", lang).replace(".trie", ".counts");
607:         let path = expand_tilde(&expanded);
608:         if !results.contains(&path) {
609:             results.push(path);
610:         }
611:     }
612: 
613:     if let Some(ref dict_path) = def.path {
614:         let expanded = dict_path.replace("{lang}", lang);
615:         let base = PathBuf::from(expand_tilde(&expanded));
616:         let dir = if base.is_dir() {
617:             base
618:         } else if let Some(parent) = base.parent() {
619:             parent.to_path_buf()
620:         } else {
621:             PathBuf::new()
622:         };
623:         if !dir.as_os_str().is_empty() {
624:             let candidate = dir.join("ngrams.counts");
625:             if !results.contains(&candidate) {
626:                 results.push(candidate);
627:             }
628:         }
629:     }
630: 
631:     for p in lp.resolve_all_marisa_ngram_counts_files() {
632:         if !results.contains(&p) {
633:             results.push(p);
634:         }
635:     }
636: 
637:     results
638: }
639: 
640: // ---------------------------------------------------------------------------
641: // Path resolution helper
642: // ---------------------------------------------------------------------------
643: 
644: fn resolve_files(def: &ResolvedBackendDef, lang: &str, lp: &LanguagePaths) -> Vec<PathBuf> {
645:     if let Some(ref explicit_path) = def.path {
646:         let expanded = explicit_path.replace("{lang}", lang);
647:         return vec![expand_tilde(&expanded)];
648:     }
649: 
650:     match def.backend_type {
651:         BackendType::File => lp.resolve_dict_files(),
652:         BackendType::Sqlite => lp.resolve_sqlite_files(),
653:         BackendType::Marisa => lp.resolve_marisa_files(),
654:         BackendType::Hunspell => Vec::new(),
655:     }
656: }
657: 
658: // ---------------------------------------------------------------------------
659: // Top-level: build an entire chain role assignment into a CachedBackend
660: // ---------------------------------------------------------------------------
661: 
662: /// The assembled result of composing a chain of backends.
663: pub struct ComposedBackend {
664:     pub loaded: bool,
665:     pub dictionary: Box<dyn DictionaryBackend>,
666:     pub spellchecker: Option<Box<dyn SpellChecker>>,
667:     pub predictor: Option<Box<dyn Predictor>>,
668: }
669: 
670: /// Compose a chain role assignment into a single backend bundle.
671: ///
672: /// `cli_system_dir` and `cli_user_dir` are CLI overrides (e.g. `--system-data-dir`).
673: /// When set, they take precedence over per-backend config values.
674: pub fn compose_chain(
675:     assignment: &super::chain::RoleAssignment,
676:     lang: &str,
677:     lp: &LanguagePaths,
678:     cli_system_dir: Option<&str>,
679:     cli_user_dir: Option<&str>,
680: ) -> ComposedBackend {
681:     let mut dict_backends: Vec<Box<dyn DictionaryBackend>> = Vec::new();
682:     let mut spellcheckers: Vec<Option<Box<dyn SpellChecker>>> = Vec::new();
683:     let mut predictors: Vec<Box<dyn Predictor>> = Vec::new();
684: 
685:     for seg in &assignment.segments {
686:         let mut seg_lp = lp.clone();
687:         // CLI overrides take precedence over per-backend config values
688:         if cli_system_dir.is_none() {
689:             if let Some(ref dir) = seg.def.system_dir {
690:                 seg_lp = seg_lp.with_system_dir(normalize_dir(dir));
691:             }
692:         }
693:         if cli_user_dir.is_none() {
694:             if let Some(ref dir) = seg.def.user_dir {
695:                 seg_lp = seg_lp.with_user_dir(normalize_dir(dir));
696:             }
697:         }
698:         // Per-backend pattern overrides always apply (no CLI equivalent yet)
699:         if seg.def.system_patterns.is_some() || seg.def.user_patterns.is_some() {
700:             seg_lp.set_patterns(
701:                 seg.def.system_patterns.as_deref(),
702:                 seg.def.user_patterns.as_deref(),
703:                 seg.def.system_patterns.as_deref(),
704:                 seg.def.user_patterns.as_deref(),
705:             );
706:         }
707:         let (dict, sc, pred) = build_backend(&seg.def, lang, &seg_lp);
708:         match seg.role {
709:             SegmentRole::Dictionary | SegmentRole::Unigrams => {
710:                 dict_backends.push(dict);
711:                 spellcheckers.push(sc);
712:                 if let Some(p) = pred {
713:                     predictors.push(p);
714:                 }
715:             }
716:             SegmentRole::Ngrams => {
717:                 if let Some(p) = pred {
718:                     predictors.push(p);
719:                 }
720:             }
721:         }
722:     }
723: 
724:     let loaded = !dict_backends.is_empty() || !predictors.is_empty();
725: 
726:     let merged_dict: Box<dyn DictionaryBackend> = if dict_backends.is_empty() {
727:         Box::new(FileDictionaryBackend::new())
728:     } else if dict_backends.len() == 1 {
729:         dict_backends.into_iter().next().unwrap()
730:     } else {
731:         eprintln!("info: merged {} dictionary backends", dict_backends.len());
732:         Box::new(MergedDictionary::new(dict_backends))
733:     };
734: 
735:     let spellchecker = spellcheckers.into_iter().flatten().next();
736: 
737:     let predictor: Option<Box<dyn Predictor>> = if predictors.is_empty() {
738:         None
739:     } else if predictors.len() == 1 {
740:         Some(predictors.into_iter().next().unwrap())
741:     } else {
742:         eprintln!("info: merged {} predictor backends", predictors.len());
743:         Some(Box::new(MergedPredictor::new(predictors)) as Box<dyn Predictor>)
744:     };
745: 
746:     ComposedBackend {
747:         loaded,
748:         dictionary: merged_dict,
749:         spellchecker,
750:         predictor,
751:     }
752: }

(End of file - total 752 lines)
</content>
```

**Tool: read**

**Input:**
```json
{}
```

**Error:**
```
Tool execution aborted
```

---

