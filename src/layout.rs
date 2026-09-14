//! Runtime keyboard-layout registry.
//!
//! Clients upload a layout (either explicit key rectangles for touch input or
//! physical rows) and receive a content-hash token. Layouts are held in a
//! bounded, session-only in-memory cache; nothing is ever written to disk.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use keyboard_layout::physical::{RowLayout, RowMetrics};
use keyboard_layout::{RectKey, RectKeyLayout};
use serde::{Deserialize, Serialize};

/// A geometric key supplied by a touch client, in the client's coordinate
/// system.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeyBox {
    pub label: String,
    #[serde(default)]
    pub alt_labels: Vec<String>,
    pub left: f32,
    pub top: f32,
    pub width: f32,
    pub height: f32,
}

/// A layout upload. Provide either explicit `keys` (touch) or `rows`
/// (physical); `keys` wins when both are present.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LayoutUpload {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keys: Option<Vec<KeyBox>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rows: Option<RowLayout>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ignored_labels: Vec<String>,
}

/// Raw upload size accepted at the D-Bus string boundary, before parsing.
pub const MAX_LAYOUT_UPLOAD_BYTES: usize = 64 * 1024;
/// Total keys accepted in one upload, across both representations and
/// including gap/spacer keys.
pub const MAX_LAYOUT_KEYS: usize = 256;
/// Physical rows accepted in one upload.
pub const MAX_LAYOUT_ROWS: usize = 32;
/// Alternate labels accepted on a single key.
pub const MAX_LAYOUT_ALTERNATES_PER_KEY: usize = 32;
/// Ignored labels accepted in one upload, counting both lists.
pub const MAX_LAYOUT_IGNORED_LABELS: usize = 256;
/// Bytes accepted in one label.
pub const MAX_LAYOUT_LABEL_BYTES: usize = 256;
/// Bytes accepted across every label of one upload.
pub const MAX_LAYOUT_LABEL_TOTAL_BYTES: usize = 32 * 1024;

fn add_label_bytes(label: &str, total: &mut usize) -> Result<(), String> {
    if label.len() > MAX_LAYOUT_LABEL_BYTES {
        return Err(format!(
            "layout label exceeds {MAX_LAYOUT_LABEL_BYTES} bytes"
        ));
    }
    *total += label.len();
    Ok(())
}

/// Reject an upload that exceeds the documented structure budgets.
///
/// Both representations are counted when both are supplied, because the
/// unused one is still deserialized and hashed into the token. The raw byte
/// bound for D-Bus strings is applied separately by the caller before JSON
/// parsing; typed callers are checked here.
pub fn validate_upload(upload: &LayoutUpload) -> Result<(), String> {
    let mut keys = 0usize;
    let mut rows = 0usize;
    let mut ignored = 0usize;
    let mut label_bytes = 0usize;

    for label in &upload.ignored_labels {
        add_label_bytes(label, &mut label_bytes)?;
        ignored += 1;
    }

    if let Some(explicit) = &upload.keys {
        keys += explicit.len();
        for key in explicit {
            add_label_bytes(&key.label, &mut label_bytes)?;
            if key.alt_labels.len() > MAX_LAYOUT_ALTERNATES_PER_KEY {
                return Err(format!(
                    "layout key has too many alternate labels (max {MAX_LAYOUT_ALTERNATES_PER_KEY})"
                ));
            }
            for label in &key.alt_labels {
                add_label_bytes(label, &mut label_bytes)?;
            }
        }
    }

    if let Some(physical) = &upload.rows {
        rows += physical.rows.len();
        for label in &physical.ignored_labels {
            add_label_bytes(label, &mut label_bytes)?;
            ignored += 1;
        }
        for row in &physical.rows {
            keys += row.keys.len();
            for key in &row.keys {
                if let Some(label) = &key.main {
                    add_label_bytes(label, &mut label_bytes)?;
                }
                if key.secondary.len() > MAX_LAYOUT_ALTERNATES_PER_KEY {
                    return Err(format!(
                        "layout key has too many secondary labels (max {MAX_LAYOUT_ALTERNATES_PER_KEY})"
                    ));
                }
                for label in &key.secondary {
                    add_label_bytes(label, &mut label_bytes)?;
                }
            }
        }
    }

    if keys > MAX_LAYOUT_KEYS {
        return Err(format!(
            "layout upload has too many keys (max {MAX_LAYOUT_KEYS})"
        ));
    }
    if rows > MAX_LAYOUT_ROWS {
        return Err(format!(
            "layout upload has too many rows (max {MAX_LAYOUT_ROWS})"
        ));
    }
    if ignored > MAX_LAYOUT_IGNORED_LABELS {
        return Err(format!(
            "layout upload has too many ignored labels (max {MAX_LAYOUT_IGNORED_LABELS})"
        ));
    }
    if label_bytes > MAX_LAYOUT_LABEL_TOTAL_BYTES {
        return Err(format!(
            "layout upload has too many label bytes (max {MAX_LAYOUT_LABEL_TOTAL_BYTES})"
        ));
    }
    Ok(())
}

/// Build a [`RectKeyLayout`] from an upload.
///
/// Uploads describe the keys a client really shows, so labels are mapped
/// exactly as uploaded: without inferred lower/upper-case variants that would
/// relocate a symbol onto another key's rectangle.
pub fn build_layout(upload: &LayoutUpload) -> Result<RectKeyLayout, String> {
    validate_upload(upload)?;
    if let Some(keys) = &upload.keys {
        if keys.is_empty() {
            return Err("layout upload contains no keys".into());
        }
        let mut rect_keys = Vec::with_capacity(keys.len());
        for key in keys {
            validate_key(key)?;
            rect_keys.push(RectKey::from_rect(
                Some(key.label.clone()),
                key.alt_labels.clone(),
                key.left,
                key.top,
                key.width,
                key.height,
            ));
        }
        let ignored: Vec<&str> = upload.ignored_labels.iter().map(String::as_str).collect();
        return Ok(RectKeyLayout::new_exact(rect_keys, &ignored));
    }

    if let Some(rows) = &upload.rows {
        let mut rows = rows.clone();
        for label in &upload.ignored_labels {
            if !rows.ignored_labels.contains(label) {
                rows.ignored_labels.push(label.clone());
            }
        }
        return Ok(rows.to_rect_key_layout_exact(&RowMetrics::default()));
    }

    Err("layout upload must contain 'keys' or 'rows'".into())
}

/// A stable content-hash token for an upload, used to deduplicate the cache.
pub fn layout_token(upload: &LayoutUpload) -> Result<String, String> {
    validate_upload(upload)?;
    let bytes = serde_json::to_vec(upload).map_err(|error| error.to_string())?;
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    Ok(format!("{hash:016x}"))
}

/// Register an upload, returning its token. Equivalent uploads share a token.
pub fn register(cache: &mut LayoutCache, upload: &LayoutUpload) -> Result<String, String> {
    let layout = Arc::new(build_layout(upload)?);
    let token = layout_token(upload)?;
    cache.insert(token.clone(), layout);
    Ok(token)
}

fn validate_key(key: &KeyBox) -> Result<(), String> {
    if key.label.is_empty() {
        return Err("layout key label must not be empty".into());
    }
    if ![key.left, key.top, key.width, key.height]
        .iter()
        .all(|value| value.is_finite())
    {
        return Err(format!("layout key '{}' has non-finite geometry", key.label));
    }
    if key.width <= 0.0 || key.height <= 0.0 {
        return Err(format!(
            "layout key '{}' must have positive width and height",
            key.label
        ));
    }
    Ok(())
}

/// Bounded, session-only LRU cache of registered layouts.
pub struct LayoutCache {
    capacity: usize,
    entries: HashMap<String, Arc<RectKeyLayout>>,
    order: VecDeque<String>,
}

impl LayoutCache {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            entries: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    pub fn get(&mut self, token: &str) -> Option<Arc<RectKeyLayout>> {
        let layout = self.entries.get(token).cloned()?;
        self.touch(token);
        Some(layout)
    }

    pub fn insert(&mut self, token: String, layout: Arc<RectKeyLayout>) {
        if self.entries.insert(token.clone(), layout).is_some() {
            self.touch(&token);
        } else {
            self.order.push_back(token);
        }
        self.evict();
    }

    pub fn forget(&mut self, token: &str) -> bool {
        self.order.retain(|entry| entry != token);
        self.entries.remove(token).is_some()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn touch(&mut self, token: &str) {
        if let Some(position) = self.order.iter().position(|entry| entry == token) {
            if let Some(entry) = self.order.remove(position) {
                self.order.push_back(entry);
            }
        }
    }

    fn evict(&mut self) {
        while self.entries.len() > self.capacity {
            match self.order.pop_front() {
                Some(oldest) => {
                    self.entries.remove(&oldest);
                }
                None => break,
            }
        }
    }
}

impl Default for LayoutCache {
    fn default() -> Self {
        Self::new(16)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use keyboard_layout::KeyboardLayout;
    use keyboard_layout::physical::{KeySpec, Row};

    fn rows_upload() -> LayoutUpload {
        LayoutUpload {
            keys: None,
            rows: Some(RowLayout::new(vec![Row::new(vec![
                KeySpec::key("q"),
                KeySpec::key("w"),
            ])])),
            ignored_labels: Vec::new(),
        }
    }

    fn keys_upload(count: usize) -> LayoutUpload {
        LayoutUpload {
            keys: Some(
                (0..count)
                    .map(|index| KeyBox {
                        label: format!("k{index}"),
                        alt_labels: Vec::new(),
                        left: 0.0,
                        top: 0.0,
                        width: 1.0,
                        height: 1.0,
                    })
                    .collect(),
            ),
            rows: None,
            ignored_labels: Vec::new(),
        }
    }

    fn rows_upload_with(rows: usize, keys_per_row: usize) -> LayoutUpload {
        LayoutUpload {
            keys: None,
            rows: Some(RowLayout::new(
                (0..rows)
                    .map(|_| {
                        Row::new(
                            (0..keys_per_row)
                                .map(|index| KeySpec::key(format!("k{index}")))
                                .collect(),
                        )
                    })
                    .collect(),
            )),
            ignored_labels: Vec::new(),
        }
    }

    #[test]
    fn builds_from_rows() {
        let layout = build_layout(&rows_upload()).unwrap();
        assert!(layout.location_of("q").is_some());
        assert!(layout.location_of("w").is_some());
    }

    #[test]
    fn builds_from_keys() {
        let upload = LayoutUpload {
            keys: Some(vec![KeyBox {
                label: "a".into(),
                alt_labels: vec!["ä".into()],
                left: 0.0,
                top: 0.0,
                width: 10.0,
                height: 20.0,
            }]),
            rows: None,
            ignored_labels: Vec::new(),
        };
        let layout = build_layout(&upload).unwrap();
        assert!(layout.location_of("a").is_some());
    }

    #[test]
    fn key_uploads_map_exactly_the_labels_they_declare() {
        // An active-Shift export: the case twins come from different keys, and
        // an apostrophe is offered only in the period key's long-press menu.
        let upload = LayoutUpload {
            keys: Some(vec![
                KeyBox {
                    label: "E".into(),
                    alt_labels: vec!["É".into()],
                    left: 0.0,
                    top: 0.0,
                    width: 10.0,
                    height: 10.0,
                },
                KeyBox {
                    label: "e".into(),
                    alt_labels: Vec::new(),
                    left: 10.0,
                    top: 0.0,
                    width: 10.0,
                    height: 10.0,
                },
                KeyBox {
                    label: ".".into(),
                    alt_labels: vec!["'".into()],
                    left: 20.0,
                    top: 0.0,
                    width: 10.0,
                    height: 10.0,
                },
            ]),
            rows: None,
            ignored_labels: Vec::new(),
        };
        let layout = build_layout(&upload).unwrap();
        let shift_e = layout.location_of("E").unwrap();
        // Declared labels answer from the keys that declare them.
        assert_eq!(layout.location_of("É").unwrap(), shift_e);
        assert_ne!(layout.location_of("e").unwrap(), shift_e);
        assert!(layout.location_of("'").is_some());
        // An undeclared case relative is not invented on any rectangle: the
        // upload, not a case conversion, decides where a symbol lives.
        assert!(layout.location_of("é").is_none());
    }

    #[test]
    fn rows_uploads_map_exactly_the_labels_they_declare() {
        let upload = LayoutUpload {
            keys: None,
            rows: Some(RowLayout::new(vec![Row::new(vec![
                KeySpec::key("Q"),
                KeySpec::key("w"),
            ])])),
            ignored_labels: Vec::new(),
        };
        let layout = build_layout(&upload).unwrap();
        assert!(layout.location_of("Q").is_some());
        assert!(layout.location_of("w").is_some());
        // Neither label gains an inferred case twin.
        assert!(layout.location_of("q").is_none());
        assert!(layout.location_of("W").is_none());
    }

    #[test]
    fn rejects_invalid_geometry() {
        let upload = LayoutUpload {
            keys: Some(vec![KeyBox {
                label: "a".into(),
                alt_labels: Vec::new(),
                left: 0.0,
                top: 0.0,
                width: 0.0,
                height: 20.0,
            }]),
            rows: None,
            ignored_labels: Vec::new(),
        };
        assert!(build_layout(&upload).is_err());
    }

    #[test]
    fn token_is_stable_and_dedupes() {
        let upload = rows_upload();
        assert_eq!(layout_token(&upload).unwrap(), layout_token(&upload).unwrap());
        let mut cache = LayoutCache::new(4);
        let first = register(&mut cache, &upload).unwrap();
        let second = register(&mut cache, &upload).unwrap();
        assert_eq!(first, second);
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn cache_evicts_the_oldest_entry() {
        let mut cache = LayoutCache::new(2);
        for variant in ["a", "b", "c"] {
            let upload = LayoutUpload {
                keys: Some(vec![KeyBox {
                    label: variant.into(),
                    alt_labels: Vec::new(),
                    left: 0.0,
                    top: 0.0,
                    width: 10.0,
                    height: 10.0,
                }]),
                rows: None,
                ignored_labels: Vec::new(),
            };
            register(&mut cache, &upload).unwrap();
        }
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn forget_removes_an_entry() {
        let upload = rows_upload();
        let mut cache = LayoutCache::new(4);
        let token = register(&mut cache, &upload).unwrap();
        assert!(cache.forget(&token));
        assert!(cache.get(&token).is_none());
    }

    #[test]
    fn upload_budgets_are_enforced_at_the_boundary() {
        // Keys, in either representation, and gaps count too.
        assert!(validate_upload(&keys_upload(MAX_LAYOUT_KEYS)).is_ok());
        assert!(validate_upload(&keys_upload(MAX_LAYOUT_KEYS + 1)).is_err());
        let gaps = LayoutUpload {
            keys: None,
            rows: Some(RowLayout::new(vec![Row::new(
                (0..MAX_LAYOUT_KEYS + 1)
                    .map(|_| KeySpec::gap(1.0))
                    .collect(),
            )])),
            ignored_labels: Vec::new(),
        };
        assert!(validate_upload(&gaps).is_err());

        // Rows.
        assert!(validate_upload(&rows_upload_with(MAX_LAYOUT_ROWS, 1)).is_ok());
        assert!(validate_upload(&rows_upload_with(MAX_LAYOUT_ROWS + 1, 1)).is_err());

        // Both representations are counted, even though `keys` wins.
        let mut both = keys_upload(MAX_LAYOUT_KEYS);
        both.rows = rows_upload_with(1, 1).rows;
        assert!(validate_upload(&both).is_err());
    }

    #[test]
    fn alternate_and_ignored_label_budgets_are_enforced() {
        let mut alternates = keys_upload(1);
        if let Some(keys) = &mut alternates.keys {
            keys[0].alt_labels = (0..MAX_LAYOUT_ALTERNATES_PER_KEY)
                .map(|index| format!("ä{index}"))
                .collect();
        }
        assert!(validate_upload(&alternates).is_ok());
        if let Some(keys) = &mut alternates.keys {
            keys[0].alt_labels.push("å".into());
        }
        assert!(validate_upload(&alternates).is_err());

        let mut ignored = keys_upload(1);
        ignored.ignored_labels = (0..MAX_LAYOUT_IGNORED_LABELS)
            .map(|index| format!("i{index}"))
            .collect();
        assert!(validate_upload(&ignored).is_ok());
        ignored.ignored_labels.push("i".into());
        assert!(validate_upload(&ignored).is_err());
    }

    #[test]
    fn label_byte_budgets_are_enforced_without_echoing_the_label() {
        let mut huge = keys_upload(1);
        if let Some(keys) = &mut huge.keys {
            keys[0].label = "x".repeat(MAX_LAYOUT_LABEL_BYTES + 1);
        }
        let error = validate_upload(&huge).unwrap_err();
        assert!(error.contains("label"), "unexpected error: {error}");
        assert!(!error.contains("xxxx"), "error echoed the oversized label");

        let mut at_limit = keys_upload(1);
        if let Some(keys) = &mut at_limit.keys {
            keys[0].label = "x".repeat(MAX_LAYOUT_LABEL_BYTES);
        }
        assert!(validate_upload(&at_limit).is_ok());

        // Exactly the aggregate budget, then one byte over it.
        let mut aggregate = LayoutUpload {
            keys: Some(Vec::new()),
            rows: None,
            ignored_labels: (0..MAX_LAYOUT_IGNORED_LABELS / 2)
                .map(|_| "x".repeat(MAX_LAYOUT_LABEL_BYTES))
                .collect(),
        };
        assert!(validate_upload(&aggregate).is_ok());
        aggregate.ignored_labels.push("x".into());
        assert!(validate_upload(&aggregate).is_err());
    }

    #[test]
    fn layout_token_and_build_reject_oversized_uploads() {
        let oversized = keys_upload(MAX_LAYOUT_KEYS + 1);

        assert!(layout_token(&oversized).is_err());
        assert!(build_layout(&oversized).is_err());
    }

    #[test]
    fn rejected_upload_leaves_the_cache_unchanged() {
        let upload = keys_upload(1);
        let mut cache = LayoutCache::new(4);
        let token = register(&mut cache, &upload).unwrap();

        assert!(register(&mut cache, &keys_upload(MAX_LAYOUT_KEYS + 1)).is_err());
        assert_eq!(cache.len(), 1);
        assert!(cache.get(&token).is_some());
    }
}
