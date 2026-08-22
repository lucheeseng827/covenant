//! The enforcement engines. One compiled contract, two data representations:
//! [`row`] validates `serde_json` records (NDJSON checks, the stream gate),
//! [`arrow`] validates `RecordBatch` columns (Parquet/CSV checks, embedding
//! in Arrow-native pipelines).

pub mod arrow;
pub mod row;

use std::collections::{HashMap, HashSet};

use crate::compile::CompiledModel;

/// Cross-record state for `unique` fields: field index → set of canonical
/// value renderings seen so far. Exact, in-memory; nulls and absent values
/// are exempt (documented in the spec).
#[derive(Debug, Default)]
pub struct UniqueTracker {
    seen: HashMap<usize, HashSet<String>>,
}

impl UniqueTracker {
    /// Build a tracker with one set per `unique` field of `model`.
    pub fn new(model: &CompiledModel) -> Self {
        let mut seen = HashMap::new();
        for (idx, field) in model.fields.iter().enumerate() {
            if field.unique {
                seen.insert(idx, HashSet::new());
            }
        }
        UniqueTracker { seen }
    }

    /// Record `key` for the field at `field_idx`; returns false when the key
    /// was already present (i.e. a uniqueness violation).
    pub fn insert(&mut self, field_idx: usize, key: &str) -> bool {
        match self.seen.get_mut(&field_idx) {
            // contains-first keeps the clean-duplicate path allocation-free;
            // the owned String is built only for genuinely new keys.
            Some(set) if set.contains(key) => false,
            Some(set) => set.insert(key.to_string()),
            // Field isn't tracked (not unique) — treat as fresh.
            None => true,
        }
    }

    /// Whether the field at `field_idx` is uniqueness-tracked.
    pub fn tracks(&self, field_idx: usize) -> bool {
        self.seen.contains_key(&field_idx)
    }
}
