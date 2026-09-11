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

/// Build a [`RectKeyLayout`] from an upload.
pub fn build_layout(upload: &LayoutUpload) -> Result<RectKeyLayout, String> {
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
        return Ok(RectKeyLayout::new(rect_keys, &ignored));
    }

    if let Some(rows) = &upload.rows {
        let mut rows = rows.clone();
        for label in &upload.ignored_labels {
            if !rows.ignored_labels.contains(label) {
                rows.ignored_labels.push(label.clone());
            }
        }
        return Ok(rows.to_rect_key_layout(&RowMetrics::default()));
    }

    Err("layout upload must contain 'keys' or 'rows'".into())
}

/// A stable content-hash token for an upload, used to deduplicate the cache.
pub fn layout_token(upload: &LayoutUpload) -> Result<String, String> {
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
}
