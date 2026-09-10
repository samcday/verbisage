---

## HeliBoard Completion Algorithm — Detailed Specification

### Architecture Overview

HeliBoard is an Android keyboard based on AOSP LatinIME. Completion/suggestion works via a **trie-based dictionary traversal with fuzzy edit-distance correction**. The pipeline is:

**Java Layer** → **JNI Bridge** → **Native C++ Dictionary + Suggest Engine**

### 1. Input Representation

The typed word is represented as a `ComposedData` containing:
- `mInputPointers`: touch coordinates (x, y, time, pointerId) per key press
- `mTypedWord`: the actual string typed
- Code points are extracted via `StringUtils.toCodePointArray()` and lowercased

When a user types, each keypress produces a `CodePoint` with coordinates. These are fed into the suggestion engine as `inputCodePoints[]`, `inputXs[]`, `inputYs[]`, `times[]`, `pointerIds[]`.

### 2. Dictionary Structure

Dictionaries are stored as **binary files** using a **Patricia Trie** data structure (version 4 format). Key types:
- **ReadOnlyBinaryDictionary**: Read-only system dictionary (`.dict` file, mmapped)
- **UserBinaryDictionary**: Expandable, backed by Android's `UserDictionary` provider
- **ContactsBinaryDictionary**: User contacts
- **AppsBinaryDictionary**: Installed app dictionaries

The dictionary structure file is loaded via `DictionaryStructureWithBufferPolicyFactory::newPolicyForExistingDictFile()`. The trie is organized as:
- **Unigram table**: Individual words with frequencies
- **Bigram table**: Word-pair frequencies for n-gram context
- **Shortcuts**: Word shortcuts

### 3. Core Suggestion Algorithm

The suggestion engine is a **best-first search** over the dictionary trie. The main entry point is `Suggest::getSuggestions()` in `suggest.cpp`.

#### Algorithm Flow:

```
1. initializeSearch() → Reset or continue search cache
2. While active nodes exist:
   a. expandCurrentDicNodes() → Expand all active nodes
   b. advanceActiveDicNodes() → Move to next priority queue
   c. advanceInputIndex() → Consume next input point
3. SuggestionsOutputUtils::outputSuggestions() → Collect and rank results
```

#### Node Expansion (`expandCurrentDicNodes`):

For each active `DicNode`, the algorithm:
1. Gets all child nodes from the dictionary trie via `DicNodeUtils::getAllChildDicNodes()`
2. For each child, determines the **ProximityType** (relationship between typed key and dictionary character):
   - `MATCH_CHAR`: Exact character match
   - `PROXIMITY_CHAR`: Key is physically near the typed key
   - `SUBSTITUTION_CHAR`: Character substitution (edit distance)
   - `ADDITIONAL_PROXIMITY_CHAR`: Extra proximity character
   - `UNRELATED_CHAR`: No relationship (pruned)

3. Processes each child based on type:
   - **`isCompletion`**: When the node's input index >= input size — the dictionary word has more characters than typed. This is the **completion path**. `processDicNodeAsMatch()` is called with `CT_COMPLETION` cost.
   - **Match**: Normal character matching
   - **Substitution**: Character replaced (Damerau-Levenshtein distance)
   - **Omission**: Character skipped (e.g., "ths" → "this")
   - **Insertion**: Extra character typed
   - **Transposition**: Adjacent characters swapped
   - **Digraph**: Composite glyph expansion (e.g., umlaut → "ue")

#### Completion Detection (`DicNode::isCompletion`):

```cpp
bool isCompletion(const int inputSize) const {
    return mDicNodeState.mDicNodeStateInput.getInputIndex(0) >= inputSize;
}
```

When the input index reaches or exceeds the number of typed characters, the node is in **completion mode**. In this state:
- `processDicNodeAsMatch()` is called for ALL children (no error correction)
- The cost is `CT_COMPLETION` with cost `COST_COMPLETION = 0.00624f` and `COST_FIRST_COMPLETION = 0.4836f`
- The first completion character has a higher cost than subsequent ones
- Language cost is 0.0f for completions

### 4. Error Correction: Damerau-Levenshtein Edit Distance

The algorithm uses **Damerau-Levenshtein distance** with these costs (from `scoring_params.cpp`):

| Operation | Cost | Notes |
|-----------|------|-------|
| Substitution | `0.3806f` | Base substitution cost |
| Omission | `0.467f` | Skip a character |
| Omission (same char) | `0.345f` | Reduced cost |
| Omission (first char) | `0.5256f` | Higher first-char cost |
| Insertion | `0.7248f` | Extra character |
| Insertion (same char) | `0.5508f` | Reduced cost |
| Insertion (proximity char) | `0.674f` | Proximity-based |
| Insertion (first char) | `0.639f` | Higher first-char cost |
| Transposition | `0.5608f` | Adjacent swap |
| Space substitution | `0.33f` | Space for char |
| Additional proximity | `0.37972f` | Nearby key |

**Edit distance threshold**: Error corrections are only allowed when `normalizedSpatialDistance < NORMALIZED_SPATIAL_DISTANCE_THRESHOLD_FOR_EDIT = 0.095f`.

### 5. Scoring System

Final scores combine **spatial distance** (touch accuracy) and **language probability** (n-gram frequency):

```
compoundDistance = spatialDistance * DISTANCE_WEIGHT_LENGTH + languageDistance * DISTANCE_WEIGHT_LANGUAGE
```

Where:
- `DISTANCE_WEIGHT_LENGTH = 0.1524f`
- `DISTANCE_WEIGHT_LANGUAGE = 1.1214f`
- Language distance uses bigram/trigram probabilities from the dictionary

**Output score formula**:
```
outputScore = TYPING_BASE_OUTPUT_SCORE + (normalizedCompoundDistance * TYPING_MAX_OUTPUT_SCORE_PER_INPUT)
```
Where `TYPING_BASE_OUTPUT_SCORE = 1.0f` and `TYPING_MAX_OUTPUT_SCORE_PER_INPUT = 0.1f`.

**Promotions** (exact/perfect match boost):
- `EXACT_MATCH_PROMOTION = 1.1f`
- `PERFECT_MATCH_PROMOTION = 1.1f`
- Case error penalty: `0.01f`
- Accent error penalty: `0.02f`
- Digraph penalty: `0.03f`

### 6. Key Constants

```cpp
MAX_SPATIAL_DISTANCE = 1.0f
MAX_CACHE_DIC_NODE_SIZE = 170 (normal), 310 (single point), 50 (low-probability locale)
THRESHOLD_SHORT_WORD_LENGTH = 4
NORMALIZED_SPATIAL_DISTANCE_THRESHOLD_FOR_EDIT = 0.095f
MIN_CONTINUOUS_SUGGESTION_INPUT_SIZE = 2
MAX_WORD_LENGTH = 45 (from DecoderSpecificConstants::DICTIONARY_MAX_WORD_LENGTH)
```

### 7. Caching System

The `DicNodesCache` manages a **priority queue** of active search nodes:
- Nodes are sorted by `compare()` which prioritizes exact matches, then lower normalized compound distance, then deeper nodes
- Cache sizes vary by input length and locale weight
- `shouldDepthLevelCache()` returns true when at cache border for typing

### 8. Multi-word Suggestions

The algorithm can suggest multiple words. When a terminal node is reached:
- `createNextWordDicNode()` creates a new root node for the next word
- `CT_NEW_WORD_SPACE_OMISSION` or `CT_NEW_WORD_SPACE_SUBSTITUTION` costs apply
- Bigram probability determines if the next word is worth exploring (`THRESHOLD_NEXT_WORD_PROBABILITY = 40`)

### 9. The `isCompletion` Path in Detail

When a DicNode reaches `isCompletion(inputSize) == true`:
1. ALL child nodes are processed via `processDicNodeAsMatch()`
2. No error correction is applied (no substitutions, omissions, insertions, transpositions)
3. The cost is `CT_COMPLETION` (spatial cost = `getCompletionCost()`, language cost = 0.0f)
4. Forward input index advances by 1 (consuming one "virtual" input point)
5. The algorithm continues traversing the trie until a terminal node is found
6. Terminal nodes are collected as completion suggestions

The `processExpandedDicNode()` also handles:
- `isSpaceSubstitutionTerminal()`: If a space is nearby, create a next-word node
- `isSpaceOmissionTerminal()`: If the word is terminal, consider space omission

### 10. Native Dictionary API (JNI Bridge)

Key JNI methods in `com_android_inputmethod_latin_BinaryDictionary.cpp`:
- `latinime_BinaryDictionary_open()` → Opens/creates a Dictionary from binary file
- `latinime_BinaryDictionary_getSuggestions()` → Main suggestion entry point
- `latinime_BinaryDictionary_isInDictionary()` → Word existence check
- `latinime_BinaryDictionary_addUnigramEntry()` → Add word to dictionary
- `latinime_BinaryDictionary_removeUnigramEntry()` → Remove word
- `latinime_BinaryDictionary_addNgramEntry()` → Add n-gram
- `latinime_BinaryDictionary_updateEntriesForWord()` → Update word entries

### 11. Suggest Policy Selection

The `TypingSuggestPolicy` is used for tap typing. It provides:
- `TypingTraversal`: Handles the traversal logic (error correction, proximity)
- `TypingScoring`: Computes spatial and language costs
- `TypingWeighting`: Applies cost weights to DicNodes

The `SuggestPolicy` interface is:
```cpp
class SuggestPolicy {
    virtual const Traversal *getTraversal() const = 0;
    virtual const Scoring *getScoring() const = 0;
    virtual const Weighting *getWeighting() const = 0;
};
```

### 12. Dictionary Format (Version 4)

The binary dictionary `.dict` file format:
- **Header**: Magic number, version, locale, attributes
- **Patricia Trie**: Nodes with code points, word IDs, terminal flags
- **Unigram table**: Word frequency data
- **Bigram table**: Word-pair frequencies
- **Shortcut table**: Word shortcuts
- **Bloom filter**: Fast existence check
- **Terminal position lookup table**: For efficient terminal node detection


---


