//! Profiles at the enforcement point: what each contract field's values
//! looked like, collected while `check` reads the data — no second pass —
//! in sketches that merge across files, batches, partitions and runs.
//!
//! A profile is the verdict's memory. `covenant check --profile` writes one,
//! `covenant profile merge` folds several into a baseline, and
//! `covenant drift` compares two. Everything in one merges: counts add,
//! extremes combine, distinct counts are HyperLogLog registers and quantiles
//! are a log-bucketed histogram ([`crate::sketch`]).
//!
//! What a profile keeps about the values, by contract type:
//! - `string`: lengths, the empty-string count and a distinct count — never
//!   a value;
//! - `integer`, `float`: min, max, mean and quantiles, plus a distinct count;
//! - `timestamp`, `date`: the earliest and latest value, and a distinct count;
//! - `uuid`: a distinct count;
//! - `boolean`, and any string or numeric field with `allowed:`: a count per
//!   value, and a count of values outside the set.
//!
//! Values are sketched in their contract type's domain, not their
//! serialization, so NDJSON, CSV and Parquet renderings of the same records
//! produce the same sketches. The one difference is inherent: a columnar
//! file cannot say a value is absent rather than null, so there every
//! record carries the column and absent values count as nulls.

use std::collections::BTreeMap;
use std::path::Path;

#[cfg(feature = "arrow")]
use arrow_array::cast::AsArray;
#[cfg(feature = "arrow")]
use arrow_array::types::{
    Date32Type, Date64Type, Float32Type, Float64Type, Int16Type, Int32Type, Int64Type, Int8Type,
    TimestampMicrosecondType, TimestampMillisecondType, TimestampNanosecondType,
    TimestampSecondType, UInt16Type, UInt32Type, UInt64Type, UInt8Type,
};
#[cfg(feature = "arrow")]
use arrow_array::{Array, RecordBatch};
#[cfg(feature = "arrow")]
use arrow_schema::{DataType, TimeUnit};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::compile::{shape, CompiledAllowed, CompiledContract, CompiledField, CompiledModel};
use crate::error::{CovenantError, Result};
use crate::sketch::{hash64, hash64_and_scalars, mix64, tail_word, Hll, Quantiles};
use crate::spec::FieldType;

/// The format revision every profile carries in its `profile:` key.
pub const PROFILE_FORMAT: u32 = 1;

/// Days from 0001-01-01 (chrono's day 1 of the common era) to 1970-01-01.
const UNIX_EPOCH_DAYS_FROM_CE: i64 = 719_163;

fn is_zero(n: &u64) -> bool {
    *n == 0
}

/// One model's profile: per-field sketches over every record read.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    /// Format revision; currently always `1`.
    pub profile: u32,
    /// The contract id and version the data was checked against.
    pub contract: String,
    pub contract_version: String,
    pub model: String,
    /// Check runs folded into this profile: 1 until profiles are merged.
    pub runs: u64,
    /// Records read, including unreadable ones.
    pub rows: u64,
    /// NDJSON lines that were not JSON objects. No field saw them.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub unreadable: u64,
    /// One entry per contract field, in contract order.
    pub fields: IndexMap<String, FieldProfile>,
}

/// What one field's values looked like.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FieldProfile {
    #[serde(rename = "type")]
    pub ty: FieldType,
    /// Readable records seen while this field was in the contract.
    pub rows: u64,
    /// Records that carried the field (NDJSON: the key; columnar: the column).
    pub present: u64,
    pub nulls: u64,
    /// Present, non-null values not of the contract type. They feed no sketch.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub invalid: u64,
    /// Floating-point NaN and infinities: of the type, but on no number line.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub non_finite: u64,
    /// Empty strings (string fields).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub empty: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub distinct: Option<Hll>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub values: Option<ValueCounts>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub numbers: Option<NumberStats>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lengths: Option<LengthStats>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub span: Option<Span>,
}

/// Exact counts for a field with a small closed set of values.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValueCounts {
    /// Every allowed value (or `true`/`false`), including those never seen.
    pub counts: BTreeMap<String, u64>,
    /// Values of the right type outside the allowed set (each one a violation).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub other: u64,
}

/// Finite numeric values.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NumberStats {
    pub count: u64,
    pub min: f64,
    pub max: f64,
    pub sum: f64,
    pub quantiles: Quantiles,
}

/// String lengths, in Unicode scalar values (as `min_length` counts them).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LengthStats {
    pub count: u64,
    pub min: u64,
    pub max: u64,
    pub sum: u64,
}

/// The earliest and latest timestamp (RFC 3339, UTC) or date (`YYYY-MM-DD`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Span {
    pub min: String,
    pub max: String,
}

impl FieldProfile {
    /// Values of the contract type: present, not null, not invalid.
    pub fn valid(&self) -> u64 {
        self.present
            .saturating_sub(self.nulls)
            .saturating_sub(self.invalid)
    }

    /// Share of records where the field was null.
    pub fn null_rate(&self) -> Option<f64> {
        ratio(self.nulls, self.rows)
    }

    /// Share of records without the field at all.
    pub fn missing_rate(&self) -> Option<f64> {
        ratio(self.rows.saturating_sub(self.present), self.rows)
    }

    /// Estimated distinct valid values: the sketch's estimate, or the exact
    /// number of categories seen (plus one for "outside the set", if any).
    pub fn distinct_estimate(&self) -> Option<f64> {
        if let Some(v) = &self.values {
            let seen = v.counts.values().filter(|&&c| c > 0).count() as f64;
            return Some(seen + if v.other > 0 { 1.0 } else { 0.0 });
        }
        self.distinct.as_ref().map(Hll::estimate)
    }

    fn merge(&mut self, other: &FieldProfile) -> std::result::Result<(), String> {
        if self.ty != other.ty {
            return Err(format!(
                "is {} in one profile and {} in the other — sketches of different types do not merge",
                self.ty.name(),
                other.ty.name()
            ));
        }
        // A timestamp or date field with valid values but no span left it
        // out (an endpoint outside years 0000–9999); so does the merge, as
        // one run over all the data would.
        let dropped = |f: &FieldProfile| {
            matches!(f.ty, FieldType::Timestamp | FieldType::Date)
                && f.span.is_none()
                && f.present > f.nulls + f.invalid
        };
        let span_dropped = dropped(self) || dropped(other);
        self.rows += other.rows;
        self.present += other.present;
        self.nulls += other.nulls;
        self.invalid += other.invalid;
        self.non_finite += other.non_finite;
        self.empty += other.empty;
        match (&mut self.distinct, &other.distinct) {
            (Some(a), Some(b)) => a.merge(b),
            (a @ None, Some(b)) => *a = Some(b.clone()),
            _ => {}
        }
        match (&mut self.values, &other.values) {
            (Some(a), Some(b)) => {
                for (k, n) in &b.counts {
                    *a.counts.entry(k.clone()).or_insert(0) += n;
                }
                a.other += b.other;
            }
            (a @ None, Some(b)) => *a = Some(b.clone()),
            _ => {}
        }
        match (&mut self.numbers, &other.numbers) {
            (Some(a), Some(b)) => {
                a.count += b.count;
                a.min = a.min.min(b.min);
                a.max = a.max.max(b.max);
                a.sum += b.sum;
                a.quantiles.merge(&b.quantiles);
            }
            (a @ None, Some(b)) => *a = Some(b.clone()),
            _ => {}
        }
        match (&mut self.lengths, &other.lengths) {
            (Some(a), Some(b)) => {
                a.count += b.count;
                a.min = a.min.min(b.min);
                a.max = a.max.max(b.max);
                a.sum += b.sum;
            }
            (a @ None, Some(b)) => *a = Some(b.clone()),
            _ => {}
        }
        let ty = self.ty;
        if span_dropped {
            self.span = None;
            return Ok(());
        }
        match (&mut self.span, &other.span) {
            (Some(a), Some(b)) => {
                let key = |s: &str| span_key(ty, s);
                if key(&b.min) < key(&a.min) {
                    a.min = b.min.clone();
                }
                if key(&b.max) > key(&a.max) {
                    a.max = b.max.clone();
                }
            }
            (a @ None, Some(b)) => *a = Some(b.clone()),
            _ => {}
        }
        Ok(())
    }

    /// Internal consistency of a profile read from disk: counts that cannot
    /// have come from a real run would make every rate derived from them lie.
    fn check(&self) -> std::result::Result<(), String> {
        if self.present > self.rows {
            return Err("present exceeds rows".into());
        }
        if self.nulls + self.invalid > self.present {
            return Err("nulls and invalid values exceed present".into());
        }
        if self.non_finite + self.empty > self.valid() {
            return Err("non-finite or empty values exceed valid values".into());
        }
        if let Some(s) = &self.span {
            if span_key(self.ty, &s.min).is_none() || span_key(self.ty, &s.max).is_none() {
                return Err(format!(
                    "span {} → {} is not a {} range",
                    s.min,
                    s.max,
                    self.ty.name()
                ));
            }
        }
        Ok(())
    }
}

fn ratio(part: u64, whole: u64) -> Option<f64> {
    (whole > 0).then(|| part as f64 / whole as f64)
}

/// Counters for a field with a small closed set of values — booleans
/// always, and string or numeric fields with `allowed:` — with the lookup
/// built once, so counting a value neither allocates nor hashes.
struct Cats {
    /// Sorted category keys; `counts[i]` belongs to `keys[i]`.
    keys: Vec<String>,
    counts: Vec<u64>,
    other: u64,
    index: CatIndex,
}

/// How a value finds its category. A hit is membership: the keys are the
/// allowed set, compared the way `CompiledAllowed` compares.
enum CatIndex {
    /// A linear scan (small sets, where it beats hashing). Keys of up to
    /// seven bytes are also packed into integers, so the common case — short
    /// codes like `USD` — compares without calling memcmp.
    Str(Vec<Option<u64>>),
    StrMap(std::collections::HashMap<String, usize>),
    Int(Vec<(i128, usize)>),
    /// By bit pattern, with `-0.0` folded into `0.0`.
    Float(Vec<(u64, usize)>),
    Bool([Option<usize>; 2]),
}

/// Beyond this many string categories, hashing beats scanning.
const CAT_SCAN_MAX: usize = 16;

impl Cats {
    fn of(field: &CompiledField) -> Option<Cats> {
        let allowed = field.allowed.as_ref();
        match field.ty {
            FieldType::Boolean => {
                let keys: Vec<bool> = [false, true]
                    .into_iter()
                    .filter(|b| allowed.is_none_or(|a| a.contains_bool(*b)))
                    .collect();
                let mut ix = [None; 2];
                for (i, b) in keys.iter().enumerate() {
                    ix[usize::from(*b)] = Some(i);
                }
                Some(Cats::new(
                    keys.iter().map(bool::to_string).collect(),
                    CatIndex::Bool(ix),
                ))
            }
            FieldType::String => {
                let mut keys: Vec<String> = match allowed? {
                    CompiledAllowed::Strings(set) => set.iter().cloned().collect(),
                    CompiledAllowed::Values(vals) => vals
                        .iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect(),
                };
                keys.sort();
                keys.dedup();
                let index = if keys.len() <= CAT_SCAN_MAX {
                    CatIndex::Str(keys.iter().map(|k| pack(k)).collect())
                } else {
                    CatIndex::StrMap(keys.iter().cloned().zip(0..).collect())
                };
                Some(Cats::new(keys, index))
            }
            FieldType::Integer => {
                let CompiledAllowed::Values(vals) = allowed? else {
                    return None;
                };
                let mut ints: Vec<i128> = vals
                    .iter()
                    .filter_map(|v| {
                        v.as_i64()
                            .map(i128::from)
                            .or_else(|| v.as_u64().map(i128::from))
                    })
                    .collect();
                ints.sort_by_key(|n| n.to_string());
                ints.dedup();
                let keys = ints.iter().map(i128::to_string).collect();
                Some(Cats::new(
                    keys,
                    CatIndex::Int(ints.into_iter().zip(0..).collect()),
                ))
            }
            FieldType::Float => {
                let CompiledAllowed::Values(vals) = allowed? else {
                    return None;
                };
                let mut floats: Vec<f64> = vals.iter().filter_map(Value::as_f64).collect();
                floats.sort_by_key(|f| float_key(*f));
                floats.dedup_by_key(|f| float_bits(*f));
                let keys = floats.iter().map(|f| float_key(*f)).collect();
                let index = floats.iter().map(|f| float_bits(*f)).zip(0..).collect();
                Some(Cats::new(keys, CatIndex::Float(index)))
            }
            _ => None,
        }
    }

    fn new(keys: Vec<String>, index: CatIndex) -> Cats {
        Cats {
            counts: vec![0; keys.len()],
            keys,
            other: 0,
            index,
        }
    }

    #[inline(always)]
    fn count(&mut self, v: Val<'_>) {
        let hit = match (&self.index, v) {
            (CatIndex::Str(packed), Val::Str(s)) => match pack(s) {
                Some(p) => packed.iter().position(|k| *k == Some(p)),
                None => self.keys.iter().position(|k| k == s),
            },
            (CatIndex::StrMap(m), Val::Str(s)) => m.get(s).copied(),
            (CatIndex::Int(ix), Val::Int(n)) => ix.iter().find(|(k, _)| *k == n).map(|(_, i)| *i),
            (CatIndex::Float(ix), Val::Float(f)) => {
                let bits = float_bits(f);
                ix.iter().find(|(k, _)| *k == bits).map(|(_, i)| *i)
            }
            (CatIndex::Bool(ix), Val::Bool(b)) => ix[usize::from(b)],
            _ => None,
        };
        match hit {
            Some(i) => self.counts[i] += 1,
            None => self.other += 1,
        }
    }

    /// A string column, counted in one loop; returns the nulls seen.
    #[cfg(feature = "arrow")]
    fn strings<'a>(&mut self, values: impl Iterator<Item = Option<&'a str>>) -> Option<u64> {
        let mut nulls = 0;
        for v in values {
            match v {
                Some(s) => self.count(Val::Str(s)),
                None => nulls += 1,
            }
        }
        Some(nulls)
    }

    fn finish(self) -> ValueCounts {
        ValueCounts {
            counts: self.keys.into_iter().zip(self.counts).collect(),
            other: self.other,
        }
    }
}

/// A string of up to seven bytes as an integer, distinct for distinct
/// strings: the bytes, then the length in the top byte, so `"a"` and
/// `"a\0"` differ.
#[inline(always)]
fn pack(s: &str) -> Option<u64> {
    let b = s.as_bytes();
    if b.len() > 7 {
        return None;
    }
    Some(tail_word(b) | (b.len() as u64) << 56)
}

/// One float, one key: `-0.0` and `0.0` are the same value.
fn float_key(f: f64) -> String {
    if f == 0.0 {
        "0".to_string()
    } else {
        f.to_string()
    }
}

fn float_bits(f: f64) -> u64 {
    if f == 0.0 {
        0
    } else {
        f.to_bits()
    }
}

/// A value in its contract type's domain.
#[derive(Clone, Copy)]
enum Val<'a> {
    Str(&'a str),
    Int(i128),
    Float(f64),
    Bool(bool),
    /// Nanoseconds since the Unix epoch, UTC.
    Instant(i128),
    /// Days since the Unix epoch.
    Day(i64),
    Uuid(u128),
}

/// Running state for one field; `span` keeps comparable keys until the end.
struct FieldAcc {
    p: FieldProfile,
    span: Option<(i128, i128)>,
    cats: Option<Cats>,
}

impl FieldAcc {
    fn new(field: &CompiledField) -> Self {
        let cats = Cats::of(field);
        FieldAcc {
            p: FieldProfile {
                ty: field.ty,
                rows: 0,
                present: 0,
                nulls: 0,
                invalid: 0,
                non_finite: 0,
                empty: 0,
                distinct: cats.is_none().then(Hll::new),
                values: None,
                numbers: None,
                lengths: None,
                span: None,
            },
            span: None,
            cats,
        }
    }

    /// One value, from any reader: the NDJSON path, and Arrow columns with
    /// no specialized loop. The column loops below do the same work with the
    /// state held in locals; the two must agree to the bit, which
    /// `tests/profile_test.rs` checks format against format.
    #[inline(always)]
    fn value(&mut self, v: Val<'_>) {
        if let Some(cats) = &mut self.cats {
            cats.count(v);
            return;
        }
        let hash = match v {
            Val::Str(s) => {
                let (hash, len) = hash64_and_scalars(s.as_bytes());
                if len == 0 {
                    self.p.empty += 1;
                }
                add_length(self.p.lengths.get_or_insert_with(LengthStats::empty), len);
                hash
            }
            Val::Int(n) => {
                add_number(
                    self.p.numbers.get_or_insert_with(NumberStats::empty),
                    int_f64(n),
                );
                hash_int(n)
            }
            Val::Float(f) => {
                if !f.is_finite() {
                    self.p.non_finite += 1;
                    return;
                }
                add_number(self.p.numbers.get_or_insert_with(NumberStats::empty), f);
                hash_float(f)
            }
            // Booleans are always counted by value, above.
            Val::Bool(b) => u64::from(b),
            Val::Instant(t) => {
                self.extend_span(t);
                hash_int(t)
            }
            Val::Day(d) => {
                self.extend_span(i128::from(d));
                hash_int(i128::from(d))
            }
            Val::Uuid(u) => hash64(&u.to_be_bytes()),
        };
        if let Some(h) = &mut self.p.distinct {
            h.insert_hash(hash);
        }
    }

    #[inline(always)]
    fn extend_span(&mut self, key: i128) {
        self.span = Some(match self.span {
            Some((lo, hi)) => (lo.min(key), hi.max(key)),
            None => (key, key),
        });
    }

    fn json(&mut self, field: &CompiledField, v: &Value) {
        let val = match field.ty {
            FieldType::String => v.as_str().map(Val::Str),
            FieldType::Integer => v
                .as_i64()
                .map(i128::from)
                .or_else(|| v.as_u64().map(i128::from))
                .map(Val::Int),
            FieldType::Float => v.as_f64().map(Val::Float),
            FieldType::Boolean => v.as_bool().map(Val::Bool),
            _ => v.as_str().and_then(|s| parse_str(field.ty, s)),
        };
        match val {
            Some(val) => self.value(val),
            None => self.p.invalid += 1,
        }
    }

    /// One Arrow column of a batch. `crate::engine::arrow` has already
    /// decided the column's type is compatible with the field's. The common
    /// column types get a loop of their own, with the sketches held in
    /// locals; everything else goes value by value.
    #[cfg(feature = "arrow")]
    fn column(&mut self, field: &CompiledField, col: &dyn Array) {
        if let Some(cats) = &mut self.cats {
            if field.ty == FieldType::String {
                let counted = match col.data_type() {
                    DataType::Utf8 => cats.strings(col.as_string::<i32>().iter()),
                    DataType::LargeUtf8 => cats.strings(col.as_string::<i64>().iter()),
                    DataType::Utf8View => cats.strings(col.as_string_view().iter()),
                    _ => None,
                };
                if let Some(nulls) = counted {
                    self.p.nulls += nulls;
                    return;
                }
            }
            return self.column_by_value(field, col);
        }
        // A column without nulls is iterated as a plain slice: no per-value
        // validity check.
        macro_rules! primitive {
            ($t:ty, $sink:ident, $conv:expr) => {{
                let a = col.as_primitive::<$t>();
                if a.null_count() == 0 {
                    self.$sink(a.values().iter().map(|&v| Some($conv(v))))
                } else {
                    self.$sink(a.iter().map(|v| v.map($conv)))
                }
            }};
        }
        macro_rules! ints {
            ($t:ty) => {
                primitive!($t, ints, i128::from)
            };
        }
        macro_rules! int_floats {
            ($t:ty) => {
                primitive!($t, floats, |n| n as f64)
            };
        }
        macro_rules! instants {
            ($t:ty, $scale:expr) => {
                primitive!($t, instants, |t| i128::from(t) * $scale)
            };
        }
        match (field.ty, col.data_type()) {
            (FieldType::String, DataType::Utf8) => self.strings(col.as_string::<i32>().iter()),
            (FieldType::String, DataType::LargeUtf8) => self.strings(col.as_string::<i64>().iter()),
            (FieldType::String, DataType::Utf8View) => self.strings(col.as_string_view().iter()),
            (FieldType::Integer, DataType::Int8) => ints!(Int8Type),
            (FieldType::Integer, DataType::Int16) => ints!(Int16Type),
            (FieldType::Integer, DataType::Int32) => ints!(Int32Type),
            (FieldType::Integer, DataType::Int64) => ints!(Int64Type),
            (FieldType::Integer, DataType::UInt8) => ints!(UInt8Type),
            (FieldType::Integer, DataType::UInt16) => ints!(UInt16Type),
            (FieldType::Integer, DataType::UInt32) => ints!(UInt32Type),
            (FieldType::Integer, DataType::UInt64) => ints!(UInt64Type),
            (FieldType::Float, DataType::Float64) => primitive!(Float64Type, floats, |f: f64| f),
            (FieldType::Float, DataType::Float32) => primitive!(Float32Type, floats, widen),
            (FieldType::Float, DataType::Int8) => int_floats!(Int8Type),
            (FieldType::Float, DataType::Int16) => int_floats!(Int16Type),
            (FieldType::Float, DataType::Int32) => int_floats!(Int32Type),
            (FieldType::Float, DataType::Int64) => int_floats!(Int64Type),
            (FieldType::Float, DataType::UInt8) => int_floats!(UInt8Type),
            (FieldType::Float, DataType::UInt16) => int_floats!(UInt16Type),
            (FieldType::Float, DataType::UInt32) => int_floats!(UInt32Type),
            (FieldType::Float, DataType::UInt64) => int_floats!(UInt64Type),
            (FieldType::Timestamp, DataType::Timestamp(TimeUnit::Second, _)) => {
                instants!(TimestampSecondType, 1_000_000_000)
            }
            (FieldType::Timestamp, DataType::Timestamp(TimeUnit::Millisecond, _)) => {
                instants!(TimestampMillisecondType, 1_000_000)
            }
            (FieldType::Timestamp, DataType::Timestamp(TimeUnit::Microsecond, _)) => {
                instants!(TimestampMicrosecondType, 1_000)
            }
            (FieldType::Timestamp, DataType::Timestamp(TimeUnit::Nanosecond, _)) => {
                instants!(TimestampNanosecondType, 1)
            }
            (FieldType::Date, DataType::Date32) => {
                instants!(Date32Type, 1)
            }
            (FieldType::Date, DataType::Date64) => {
                primitive!(Date64Type, instants, |ms: i64| i128::from(
                    ms.div_euclid(86_400_000)
                ))
            }
            _ => self.column_by_value(field, col),
        }
    }

    #[cfg(feature = "arrow")]
    fn ints(&mut self, values: impl Iterator<Item = Option<i128>>) {
        let p = &mut self.p;
        let hll = p
            .distinct
            .as_mut()
            .expect("uncategorized fields keep a distinct sketch");
        let mut s = p.numbers.take().unwrap_or_else(NumberStats::empty);
        let mut nulls = 0;
        for v in values {
            match v {
                Some(n) => {
                    add_number(&mut s, int_f64(n));
                    hll.insert_hash(hash_int(n));
                }
                None => nulls += 1,
            }
        }
        p.nulls += nulls;
        if s.count > 0 {
            p.numbers = Some(s);
        }
    }

    #[cfg(feature = "arrow")]
    fn floats(&mut self, values: impl Iterator<Item = Option<f64>>) {
        let p = &mut self.p;
        let hll = p
            .distinct
            .as_mut()
            .expect("uncategorized fields keep a distinct sketch");
        let mut s = p.numbers.take().unwrap_or_else(NumberStats::empty);
        let (mut nulls, mut non_finite) = (0, 0);
        for v in values {
            match v {
                Some(f) if f.is_finite() => {
                    add_number(&mut s, f);
                    hll.insert_hash(hash_float(f));
                }
                Some(_) => non_finite += 1,
                None => nulls += 1,
            }
        }
        p.nulls += nulls;
        p.non_finite += non_finite;
        if s.count > 0 {
            p.numbers = Some(s);
        }
    }

    #[cfg(feature = "arrow")]
    fn strings<'a>(&mut self, values: impl Iterator<Item = Option<&'a str>>) {
        let p = &mut self.p;
        let hll = p
            .distinct
            .as_mut()
            .expect("uncategorized fields keep a distinct sketch");
        let mut l = p.lengths.take().unwrap_or_else(LengthStats::empty);
        let (mut nulls, mut empty) = (0, 0);
        for v in values {
            match v {
                Some(s) => {
                    let (hash, len) = hash64_and_scalars(s.as_bytes());
                    if len == 0 {
                        empty += 1;
                    }
                    add_length(&mut l, len);
                    hll.insert_hash(hash);
                }
                None => nulls += 1,
            }
        }
        p.nulls += nulls;
        p.empty += empty;
        if l.count > 0 {
            p.lengths = Some(l);
        }
    }

    /// Timestamps (nanoseconds) and dates (days), as span keys.
    #[cfg(feature = "arrow")]
    fn instants(&mut self, values: impl Iterator<Item = Option<i128>>) {
        let p = &mut self.p;
        let hll = p
            .distinct
            .as_mut()
            .expect("uncategorized fields keep a distinct sketch");
        let (mut lo, mut hi) = self.span.unwrap_or((i128::MAX, i128::MIN));
        let mut nulls = 0;
        for v in values {
            match v {
                Some(t) => {
                    lo = lo.min(t);
                    hi = hi.max(t);
                    hll.insert_hash(hash_int(t));
                }
                None => nulls += 1,
            }
        }
        p.nulls += nulls;
        if lo <= hi {
            self.span = Some((lo, hi));
        }
    }

    #[cfg(feature = "arrow")]
    fn column_by_value(&mut self, field: &CompiledField, col: &dyn Array) {
        macro_rules! each {
            ($arr:expr, $conv:expr) => {{
                let a = $arr;
                for i in 0..a.len() {
                    if a.is_null(i) {
                        self.p.nulls += 1;
                        continue;
                    }
                    #[allow(clippy::redundant_closure_call)]
                    let v = $conv(a.value(i));
                    match v {
                        Some(v) => self.value(v),
                        None => self.p.invalid += 1,
                    }
                }
            }};
        }
        let ty = field.ty;
        let int = |n: i128| {
            Some(if ty == FieldType::Float {
                Val::Float(int_f64(n))
            } else {
                Val::Int(n)
            })
        };
        match col.data_type() {
            DataType::Utf8 if ty == FieldType::String => {
                each!(col.as_string::<i32>(), |s| Some(Val::Str(s)))
            }
            DataType::Utf8 => each!(col.as_string::<i32>(), |s| parse_str(ty, s)),
            DataType::LargeUtf8 => each!(col.as_string::<i64>(), |s| parse_str(ty, s)),
            DataType::Utf8View => each!(col.as_string_view(), |s| parse_str(ty, s)),
            DataType::Boolean => each!(col.as_boolean(), |b| Some(Val::Bool(b))),
            DataType::Int8 => each!(col.as_primitive::<Int8Type>(), |v: i8| int(v.into())),
            DataType::Int16 => each!(col.as_primitive::<Int16Type>(), |v: i16| int(v.into())),
            DataType::Int32 => each!(col.as_primitive::<Int32Type>(), |v: i32| int(v.into())),
            DataType::Int64 => each!(col.as_primitive::<Int64Type>(), |v: i64| int(v.into())),
            DataType::UInt8 => each!(col.as_primitive::<UInt8Type>(), |v: u8| int(v.into())),
            DataType::UInt16 => each!(col.as_primitive::<UInt16Type>(), |v: u16| int(v.into())),
            DataType::UInt32 => each!(col.as_primitive::<UInt32Type>(), |v: u32| int(v.into())),
            DataType::UInt64 => each!(col.as_primitive::<UInt64Type>(), |v: u64| int(v.into())),
            DataType::Float32 => {
                each!(col.as_primitive::<Float32Type>(), |v: f32| Some(
                    Val::Float(widen(v))
                ))
            }
            DataType::Float64 => {
                each!(col.as_primitive::<Float64Type>(), |v: f64| Some(
                    Val::Float(v)
                ))
            }
            DataType::Timestamp(unit, _) => {
                let scale: i128 = match unit {
                    TimeUnit::Second => 1_000_000_000,
                    TimeUnit::Millisecond => 1_000_000,
                    TimeUnit::Microsecond => 1_000,
                    TimeUnit::Nanosecond => 1,
                };
                let at = |v: i64| Some(Val::Instant(i128::from(v) * scale));
                match unit {
                    TimeUnit::Second => each!(col.as_primitive::<TimestampSecondType>(), at),
                    TimeUnit::Millisecond => {
                        each!(col.as_primitive::<TimestampMillisecondType>(), at)
                    }
                    TimeUnit::Microsecond => {
                        each!(col.as_primitive::<TimestampMicrosecondType>(), at)
                    }
                    TimeUnit::Nanosecond => {
                        each!(col.as_primitive::<TimestampNanosecondType>(), at)
                    }
                }
            }
            DataType::Date32 => {
                each!(col.as_primitive::<Date32Type>(), |v: i32| Some(Val::Day(
                    v.into()
                )))
            }
            DataType::Date64 => each!(col.as_primitive::<Date64Type>(), |v: i64| Some(Val::Day(
                v.div_euclid(86_400_000)
            ))),
            DataType::FixedSizeBinary(16) => each!(col.as_fixed_size_binary(), |b: &[u8]| {
                <[u8; 16]>::try_from(b)
                    .ok()
                    .map(|b| Val::Uuid(u128::from_be_bytes(b)))
            }),
            // Compatibility admits nothing else; count rather than guess.
            _ => {
                let nulls = col.logical_null_count() as u64;
                self.p.nulls += nulls;
                self.p.invalid += col.len() as u64 - nulls;
            }
        }
    }

    fn finish(mut self) -> FieldProfile {
        self.p.values = self.cats.map(Cats::finish);
        if let Some((lo, hi)) = self.span {
            let (min, max) = (render_key(self.p.ty, lo), render_key(self.p.ty, hi));
            // An endpoint outside years 0000–9999 (in UTC) has no text form a
            // profile reads back, so such a span is left out: the field keeps
            // its counts and sketches, and the file stays readable.
            if span_key(self.p.ty, &min).is_some() && span_key(self.p.ty, &max).is_some() {
                self.p.span = Some(Span { min, max });
            }
        }
        self.p
    }
}

/// An `f32` widened through its shortest decimal rendering, so a Parquet
/// `FLOAT` 0.1 profiles as the 0.1 an NDJSON file holds, not
/// 0.100000001490116.
#[cfg(feature = "arrow")]
fn widen(v: f32) -> f64 {
    if v.is_finite() {
        v.to_string().parse().unwrap_or(f64::from(v))
    } else {
        f64::from(v)
    }
}

impl NumberStats {
    /// Before the first value: every value is a new min and max.
    fn empty() -> NumberStats {
        NumberStats {
            count: 0,
            min: f64::INFINITY,
            max: f64::NEG_INFINITY,
            sum: 0.0,
            quantiles: Quantiles::new(),
        }
    }
}

impl LengthStats {
    fn empty() -> LengthStats {
        LengthStats {
            count: 0,
            min: u64::MAX,
            max: 0,
            sum: 0,
        }
    }
}

/// `x` is finite, so plain comparisons do: `f64::min`'s NaN handling is
/// dead weight on this path.
#[inline(always)]
fn add_number(s: &mut NumberStats, x: f64) {
    s.count += 1;
    if x < s.min {
        s.min = x;
    }
    if x > s.max {
        s.max = x;
    }
    s.sum += x;
    s.quantiles.insert(x);
}

#[inline(always)]
fn add_length(l: &mut LengthStats, len: u64) {
    l.count += 1;
    l.min = l.min.min(len);
    l.max = l.max.max(len);
    l.sum += len;
}

/// Through i64 where the value fits: i128 → f64 is a software routine.
#[inline(always)]
fn int_f64(n: i128) -> f64 {
    match i64::try_from(n) {
        Ok(i) => i as f64,
        Err(_) => n as f64,
    }
}

/// Seeds keep the hash families of different domains apart.
const INT_SEED: u64 = 0x2545_F491_4F6C_DD1D;
const FLOAT_SEED: u64 = 0x9FB2_1C65_1E98_DF25;

/// Integers, instants and days: one 64-bit mix, a perfect hash of anything
/// that fits in 64 bits; wider values hash their bytes. Pinned like
/// [`hash64`]: persisted distinct counts depend on it.
#[inline(always)]
fn hash_int(n: i128) -> u64 {
    match i64::try_from(n) {
        Ok(i) => mix64(i as u64 ^ INT_SEED),
        Err(_) => hash64(&n.to_le_bytes()),
    }
}

#[inline(always)]
fn hash_float(f: f64) -> u64 {
    mix64(float_bits(f) ^ FLOAT_SEED)
}

/// A string rendering of a timestamp, date or uuid, in its domain.
fn parse_str(ty: FieldType, s: &str) -> Option<Val<'_>> {
    match ty {
        FieldType::String => Some(Val::Str(s)),
        FieldType::Timestamp => instant(s).map(Val::Instant),
        FieldType::Date => day(s).map(Val::Day),
        FieldType::Uuid => uuid(s).map(Val::Uuid),
        _ => None,
    }
}

/// Nanoseconds since the epoch for an RFC 3339 timestamp: the common shape
/// by hand, anything else by chrono — which also decides what is valid.
fn instant(s: &str) -> Option<i128> {
    if let Some(t) = fast_instant(s.as_bytes()) {
        return Some(t);
    }
    let t = chrono::DateTime::parse_from_rfc3339(s).ok()?;
    Some(i128::from(t.timestamp()) * 1_000_000_000 + i128::from(t.timestamp_subsec_nanos()))
}

/// `YYYY-MM-DDTHH:MM:SS[.f{1,9}](Z|±HH:MM)`, fully validated. `None` means
/// "not this shape", never "invalid": the caller then asks chrono. So this
/// may only accept what chrono accepts, to the nanosecond.
fn fast_instant(b: &[u8]) -> Option<i128> {
    let head: &[u8; 20] = b.get(..20)?.try_into().ok()?;
    if head[10] != b'T' || head[13] != b':' || head[16] != b':' {
        return None;
    }
    let days = civil_days(&head[..10])?;
    let (hour, min, sec) = (two(head, 11)?, two(head, 14)?, two(head, 17)?);
    if hour > 23 || min > 59 || sec > 59 {
        return None;
    }
    let mut i = 19;
    let mut nanos: i64 = 0;
    if b[i] == b'.' {
        i += 1;
        let start = i;
        while i < b.len() && b[i].is_ascii_digit() {
            // Past nine digits the shape is refused below; stop accumulating
            // so a long run of digits cannot overflow.
            if i - start < 9 {
                nanos = nanos * 10 + i64::from(b[i] - b'0');
            }
            i += 1;
        }
        let digits = i - start;
        if digits == 0 || digits > 9 {
            return None;
        }
        nanos *= 10i64.pow((9 - digits) as u32);
    }
    let offset = match &b[i..] {
        [b'Z'] => 0,
        [sign @ (b'+' | b'-'), _, _, b':', _, _] => {
            let (oh, om) = (two(b, i + 1)?, two(b, i + 4)?);
            if oh > 23 || om > 59 {
                return None;
            }
            let secs = oh * 3_600 + om * 60;
            if *sign == b'-' {
                -secs
            } else {
                secs
            }
        }
        _ => return None,
    };
    let secs = days * 86_400 + hour * 3_600 + min * 60 + sec - offset;
    Some(i128::from(secs) * 1_000_000_000 + i128::from(nanos))
}

/// Days since the epoch for a `YYYY-MM-DD` calendar date — the exact set
/// `shape::is_date` accepts, without chrono's general parser.
fn day(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() != 10 {
        return None;
    }
    civil_days(b)
}

/// `YYYY-MM-DD` (exactly ten bytes, a real calendar date) → days since
/// 1970-01-01, by Howard Hinnant's days-from-civil.
fn civil_days(b: &[u8]) -> Option<i64> {
    let b: &[u8; 10] = b.try_into().ok()?;
    if b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    let year = two(b, 0)? * 100 + two(b, 2)?;
    let (month, day) = (two(b, 5)?, two(b, 8)?);
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days_in_month = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return None,
    };
    if day < 1 || day > days_in_month {
        return None;
    }
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146_097 + doe - 719_468)
}

/// Two ASCII digits at `i`, as a number.
#[inline(always)]
fn two(b: &[u8], i: usize) -> Option<i64> {
    let (hi, lo) = (
        b.get(i)?.wrapping_sub(b'0'),
        b.get(i + 1)?.wrapping_sub(b'0'),
    );
    (hi < 10 && lo < 10).then(|| i64::from(hi) * 10 + i64::from(lo))
}

fn uuid(s: &str) -> Option<u128> {
    if !shape::is_uuid(s) {
        return None;
    }
    let hex: String = s.chars().filter(|c| *c != '-').collect();
    u128::from_str_radix(&hex, 16).ok()
}

/// The comparable key of a span endpoint.
fn span_key(ty: FieldType, s: &str) -> Option<i128> {
    match ty {
        FieldType::Timestamp => instant(s),
        FieldType::Date => day(s).map(i128::from),
        _ => None,
    }
}

fn render_key(ty: FieldType, key: i128) -> String {
    match ty {
        FieldType::Date => i32::try_from(i64::try_from(key).unwrap_or(0) + UNIX_EPOCH_DAYS_FROM_CE)
            .ok()
            .and_then(chrono::NaiveDate::from_num_days_from_ce_opt)
            .map(|d| d.format("%Y-%m-%d").to_string())
            .unwrap_or_default(),
        _ => {
            let secs = key.div_euclid(1_000_000_000);
            let nanos = key.rem_euclid(1_000_000_000) as u32;
            i64::try_from(secs)
                .ok()
                .and_then(|s| chrono::DateTime::from_timestamp(s, nanos))
                .map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true))
                .unwrap_or_default()
        }
    }
}

/// Builds a [`Profile`] while a check reads the data. Feed it the same
/// records or batches the engines validate, then [`Profiler::finish`].
pub struct Profiler {
    contract: String,
    contract_version: String,
    model: String,
    rows: u64,
    unreadable: u64,
    fields: Vec<(String, FieldAcc)>,
}

impl Profiler {
    pub fn new(contract: &CompiledContract, model: &CompiledModel) -> Self {
        Profiler {
            contract: contract.id.clone(),
            contract_version: contract.version.clone(),
            model: model.name.clone(),
            rows: 0,
            unreadable: 0,
            fields: model
                .fields
                .iter()
                .map(|f| (f.name.clone(), FieldAcc::new(f)))
                .collect(),
        }
    }

    /// One NDJSON record. `model` must be the model the profiler was built for.
    pub fn observe_record(&mut self, model: &CompiledModel, record: &Value) {
        let Some(obj) = record.as_object() else {
            self.observe_unreadable();
            return;
        };
        self.rows += 1;
        for (idx, field) in model.fields.iter().enumerate() {
            self.observe_field(field, idx, obj.get(&field.name));
        }
    }

    /// Start a readable record whose fields arrive through
    /// [`Profiler::observe_field`] — the row engine's observer, which hands
    /// over each value as validation looks it up.
    pub fn start_record(&mut self) {
        self.rows += 1;
    }

    /// One field of the current record: `field` is `model.fields[idx]`,
    /// `value` is `None` when the record lacks it.
    #[inline]
    pub fn observe_field(&mut self, field: &CompiledField, idx: usize, value: Option<&Value>) {
        let acc = &mut self.fields[idx].1;
        acc.p.rows += 1;
        match value {
            None => {}
            Some(Value::Null) => {
                acc.p.present += 1;
                acc.p.nulls += 1;
            }
            Some(v) => {
                acc.p.present += 1;
                acc.json(field, v);
            }
        }
    }

    /// A line that did not parse as JSON.
    pub fn observe_unreadable(&mut self) {
        self.rows += 1;
        self.unreadable += 1;
    }

    /// One Arrow batch. `model` must be the model the profiler was built for.
    #[cfg(feature = "arrow")]
    pub fn observe_batch(&mut self, model: &CompiledModel, batch: &RecordBatch) {
        let n = batch.num_rows() as u64;
        self.rows += n;
        let schema = batch.schema();
        for ((_, acc), field) in self.fields.iter_mut().zip(&model.fields) {
            acc.p.rows += n;
            let Some((idx, _)) = schema.column_with_name(&field.name) else {
                continue;
            };
            let col = batch.column(idx);
            acc.p.present += n;
            if crate::engine::arrow::type_compatible(field.ty, col.data_type()) {
                acc.column(field, col.as_ref());
            } else {
                let nulls = col.logical_null_count() as u64;
                acc.p.nulls += nulls;
                acc.p.invalid += n - nulls;
            }
        }
    }

    pub fn finish(self) -> Profile {
        Profile {
            profile: PROFILE_FORMAT,
            contract: self.contract,
            contract_version: self.contract_version,
            model: self.model,
            runs: 1,
            rows: self.rows,
            unreadable: self.unreadable,
            fields: self
                .fields
                .into_iter()
                .map(|(name, acc)| (name, acc.finish()))
                .collect(),
        }
    }
}

impl Profile {
    /// Read a profile file, refusing formats and contents this release
    /// cannot vouch for.
    pub fn from_path(path: &Path) -> Result<Profile> {
        let text = std::fs::read_to_string(path).map_err(|e| CovenantError::Io {
            path: path.display().to_string(),
            source: e,
        })?;
        Profile::parse(&text, &path.display().to_string())
    }

    pub fn parse(text: &str, origin: &str) -> Result<Profile> {
        let invalid = |message: String| CovenantError::ProfileInvalid {
            path: origin.to_string(),
            message,
        };
        let version = serde_json::from_str::<Value>(text)
            .map_err(|e| invalid(format!("not JSON: {e}")))?
            .get("profile")
            .and_then(Value::as_u64);
        if version != Some(u64::from(PROFILE_FORMAT)) {
            return Err(invalid(match version {
                Some(v) => format!(
                    "profile format {v} is not supported (this release reads {PROFILE_FORMAT})"
                ),
                None => "not a profile: no `profile:` format key".into(),
            }));
        }
        let p: Profile = serde_json::from_str(text).map_err(|e| invalid(e.to_string()))?;
        for (name, f) in &p.fields {
            if f.rows > p.rows {
                return Err(invalid(format!(
                    "field {name:?}: rows exceed the profile's rows"
                )));
            }
            f.check()
                .map_err(|m| invalid(format!("field {name:?}: {m}")))?;
        }
        Ok(p)
    }

    /// Fold `other` in: the profile of both runs' data. Both must describe
    /// the same contract model; a field whose type changed between them
    /// cannot merge. The newer contract version is kept.
    pub fn merge(&mut self, other: &Profile) -> Result<()> {
        if self.contract != other.contract || self.model != other.model {
            return Err(CovenantError::ProfileInvalid {
                path: format!("{}/{}", other.contract, other.model),
                message: format!(
                    "cannot merge into a profile of {}/{}: profiles merge only within one contract model",
                    self.contract, self.model
                ),
            });
        }
        for (name, f) in &other.fields {
            if let Some(mine) = self.fields.get(name) {
                if mine.ty != f.ty {
                    return Err(CovenantError::ProfileInvalid {
                        path: format!("{}/{}", self.contract, self.model),
                        message: format!(
                            "field {name:?} is {} in one profile and {} in the other — \
                             sketches of different types do not merge",
                            mine.ty.name(),
                            f.ty.name()
                        ),
                    });
                }
            }
        }
        for (name, f) in &other.fields {
            match self.fields.get_mut(name) {
                Some(mine) => mine.merge(f).expect("types checked above"),
                None => {
                    self.fields.insert(name.clone(), f.clone());
                }
            }
        }
        let newer = match (
            semver::Version::parse(&self.contract_version),
            semver::Version::parse(&other.contract_version),
        ) {
            (Ok(a), Ok(b)) => b > a,
            _ => false,
        };
        if newer {
            self.contract_version = other.contract_version.clone();
        }
        self.runs += other.runs;
        self.rows += other.rows;
        self.unreadable += other.unreadable;
        Ok(())
    }

    /// Serialize compactly: sketches make pretty-printing useless to a reader
    /// (`covenant profile show` is the readable form).
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("profiles serialize")
    }

    /// Write atomically (a temporary file, then a rename), so a reader never
    /// sees half a profile.
    pub fn write(&self, path: &Path) -> Result<()> {
        let io = |e| CovenantError::Io {
            path: path.display().to_string(),
            source: e,
        };
        let mut tmp = path.as_os_str().to_owned();
        tmp.push(".tmp");
        std::fs::write(&tmp, self.to_json() + "\n").map_err(io)?;
        std::fs::rename(&tmp, path).map_err(io)
    }

    /// The numbers a reader wants from each field, computed from the sketches.
    pub fn summary(&self) -> ProfileSummary {
        ProfileSummary {
            contract: self.contract.clone(),
            contract_version: self.contract_version.clone(),
            model: self.model.clone(),
            runs: self.runs,
            rows: self.rows,
            unreadable: self.unreadable,
            fields: self
                .fields
                .iter()
                .map(|(name, f)| FieldSummary::of(name, f))
                .collect(),
        }
    }
}

/// [`Profile::summary`]: rates, estimates and quantiles instead of sketches.
#[derive(Debug, Clone, Serialize)]
pub struct ProfileSummary {
    pub contract: String,
    pub contract_version: String,
    pub model: String,
    pub runs: u64,
    pub rows: u64,
    #[serde(skip_serializing_if = "is_zero")]
    pub unreadable: u64,
    pub fields: Vec<FieldSummary>,
}

/// One field of a [`ProfileSummary`].
#[derive(Debug, Clone, Serialize)]
pub struct FieldSummary {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: FieldType,
    pub rows: u64,
    pub present_rate: Option<f64>,
    pub null_rate: Option<f64>,
    #[serde(skip_serializing_if = "is_zero")]
    pub invalid: u64,
    #[serde(skip_serializing_if = "is_zero")]
    pub non_finite: u64,
    #[serde(skip_serializing_if = "is_zero")]
    pub empty: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub distinct: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub values: Option<ValueCounts>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub numbers: Option<NumberSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lengths: Option<LengthSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub span: Option<Span>,
}

#[derive(Debug, Clone, Serialize)]
pub struct NumberSummary {
    pub min: f64,
    pub p05: f64,
    pub p25: f64,
    pub p50: f64,
    pub p75: f64,
    pub p95: f64,
    pub max: f64,
    pub mean: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct LengthSummary {
    pub min: u64,
    pub max: u64,
    pub mean: f64,
}

impl FieldSummary {
    fn of(name: &str, f: &FieldProfile) -> FieldSummary {
        FieldSummary {
            name: name.to_string(),
            ty: f.ty,
            rows: f.rows,
            present_rate: ratio(f.present, f.rows),
            null_rate: f.null_rate(),
            invalid: f.invalid,
            non_finite: f.non_finite,
            empty: f.empty,
            distinct: f.distinct_estimate().map(f64::round),
            values: f.values.clone(),
            numbers: f.numbers.as_ref().map(|n| {
                let q = |p: f64| n.quantiles.quantile(p).unwrap_or(n.min);
                NumberSummary {
                    min: n.min,
                    p05: q(0.05),
                    p25: q(0.25),
                    p50: q(0.5),
                    p75: q(0.75),
                    p95: q(0.95),
                    max: n.max,
                    mean: n.sum / n.count.max(1) as f64,
                }
            }),
            lengths: f.lengths.as_ref().map(|l| LengthSummary {
                min: l.min,
                max: l.max,
                mean: l.sum as f64 / l.count.max(1) as f64,
            }),
            span: f.span.clone(),
        }
    }
}

impl ProfileSummary {
    /// A table for terminals: one line per field.
    pub fn render_human(&self) -> String {
        use std::fmt::Write;
        let mut s = String::new();
        let _ = writeln!(
            s,
            "{} v{} · model {} · {} row{} in {} run{}{}",
            self.contract,
            self.contract_version,
            self.model,
            group(self.rows),
            if self.rows == 1 { "" } else { "s" },
            self.runs,
            if self.runs == 1 { "" } else { "s" },
            if self.unreadable > 0 {
                format!(" ({} unreadable)", group(self.unreadable))
            } else {
                String::new()
            }
        );
        let name_w = self
            .fields
            .iter()
            .map(|f| f.name.len())
            .max()
            .unwrap_or(0)
            .max(5);
        let _ = writeln!(
            s,
            "  {:<name_w$}  {:<9}  {:>7}  {:>6}  {:>9}  summary",
            "field", "type", "present", "null", "distinct"
        );
        for f in &self.fields {
            let distinct = match (&f.values, f.distinct) {
                (Some(_), Some(d)) => format!("{d} values"),
                (None, Some(d)) => format!("~{}", group(d as u64)),
                _ => "—".into(),
            };
            let line = format!(
                "  {:<name_w$}  {:<9}  {:>7}  {:>6}  {:>9}  {}",
                f.name,
                f.ty.name(),
                pct(f.present_rate),
                pct(f.null_rate),
                distinct,
                f.describe()
            );
            let _ = writeln!(s, "{}", line.trim_end());
        }
        s
    }
}

impl FieldSummary {
    fn describe(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if let Some(n) = &self.numbers {
            parts.push(format!(
                "min {} · p50 {} · p95 {} · max {} · mean {}",
                num(n.min),
                num(n.p50),
                num(n.p95),
                num(n.max),
                num(n.mean)
            ));
        }
        if let Some(l) = &self.lengths {
            if l.min == l.max {
                parts.push(format!("length {}", l.min));
            } else {
                parts.push(format!("length {}–{} (mean {})", l.min, l.max, num(l.mean)));
            }
        }
        if let Some(sp) = &self.span {
            parts.push(format!("{} → {}", sp.min, sp.max));
        }
        if let Some(v) = &self.values {
            let total: u64 = v.counts.values().sum::<u64>() + v.other;
            let mut counts: Vec<(&String, &u64)> = v.counts.iter().collect();
            counts.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
            let shown: Vec<String> = counts
                .iter()
                .take(5)
                .map(|(k, n)| format!("{k} {}", pct(ratio(**n, total))))
                .collect();
            let mut line = shown.join(" · ");
            if counts.len() > 5 {
                line.push_str(&format!(" · {} more", counts.len() - 5));
            }
            if v.other > 0 {
                line.push_str(&format!(" · {} outside the set", group(v.other)));
            }
            parts.push(line);
        }
        if self.empty > 0 {
            parts.push(format!("{} empty", group(self.empty)));
        }
        if self.invalid > 0 {
            parts.push(format!("{} not {}", group(self.invalid), self.ty.name()));
        }
        if self.non_finite > 0 {
            parts.push(format!("{} NaN/infinite", group(self.non_finite)));
        }
        parts.join(" · ")
    }
}

fn pct(r: Option<f64>) -> String {
    match r {
        None => "—".into(),
        Some(r) if r > 0.0 && r < 0.001 => "<0.1%".into(),
        Some(r) => {
            let s = format!("{:.1}", r * 100.0);
            format!("{}%", s.trim_end_matches(".0"))
        }
    }
}

/// A number for people: integers without a fraction, others to four
/// significant places.
fn num(x: f64) -> String {
    let digits = if x.fract() == 0.0 {
        0
    } else {
        (4 - x.abs().log10().floor() as i32 - 1).clamp(0, 6) as usize
    };
    if digits == 0 && x.abs() < 1e15 {
        let g = group(x.abs().round() as u64);
        return if x < 0.0 && g != "0" {
            format!("-{g}")
        } else {
            g
        };
    }
    let s = format!("{x:.digits$}");
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s
    }
}

/// Thousands separators: 1234567 → 1,234,567.
fn group(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn by_chrono(s: &str) -> Option<i128> {
        chrono::DateTime::parse_from_rfc3339(s).ok().map(|t| {
            i128::from(t.timestamp()) * 1_000_000_000 + i128::from(t.timestamp_subsec_nanos())
        })
    }

    #[test]
    fn the_fast_timestamp_path_agrees_with_chrono() {
        let mut cases: Vec<String> = [
            "2026-09-01T00:00:00Z",
            "1970-01-01T00:00:00Z",
            "1969-12-31T23:59:59.999999999Z",
            "0000-03-01T00:00:00Z",
            "9999-12-31T23:59:59Z",
            "2024-02-29T12:00:00+05:30",
            "2023-02-29T12:00:00Z",
            "2100-02-29T00:00:00Z",
            "2000-02-29T00:00:00-00:00",
            "2026-04-31T00:00:00Z",
            "2026-13-01T00:00:00Z",
            "2026-00-10T00:00:00Z",
            "2026-09-00T00:00:00Z",
            "2026-09-01T24:00:00Z",
            "2026-09-01T23:60:00Z",
            "2016-12-31T23:59:60Z",
            "2026-09-01T10:00:00.Z",
            "2026-09-01T10:00:00.1234567891Z",
            "2026-09-01t10:00:00z",
            "2026-09-01 10:00:00Z",
            "2026-09-01T10:00:00+24:00",
            "2026-09-01T10:00:00+23:59",
            "2026-09-01T10:00:00-08:00",
            "2026-09-01T10:00:00+0800",
            "2026-09-01T10:00:00",
            "2026-09-01T10:00:00Zjunk",
            "2026-9-01T10:00:00Z",
            "+2026-09-01T10:00:00Z",
            "",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        for digits in 1..=9 {
            cases.push(format!("2026-09-01T10:00:00.{}Z", "7".repeat(digits)));
        }
        for m in 1..=12 {
            for d in [1, 28, 29, 30, 31] {
                cases.push(format!("2024-{m:02}-{d:02}T01:02:03.5+01:00"));
                cases.push(format!("2025-{m:02}-{d:02}T01:02:03Z"));
            }
        }
        for s in &cases {
            assert_eq!(instant(s), by_chrono(s), "{s:?}");
            if let Some(fast) = fast_instant(s.as_bytes()) {
                assert_eq!(Some(fast), by_chrono(s), "fast path accepted {s:?}");
            }
        }
    }

    #[test]
    fn the_fast_date_path_accepts_exactly_what_the_engine_does() {
        let mut cases = vec![
            "2026-09-01",
            "2024-02-29",
            "2023-02-29",
            "1900-02-29",
            "2000-02-29",
            "0000-01-01",
            "9999-12-31",
            "2026-9-01",
            "2026-09-1",
            " 2026-09-01",
            "2026-09-01 ",
            "2026/09/01",
            "20260901",
            "2026-13-01",
            "2026-00-01",
            "2026-06-31",
            "",
        ]
        .into_iter()
        .map(String::from)
        .collect::<Vec<_>>();
        for m in 1..=12 {
            for d in 28..=32 {
                cases.push(format!("2025-{m:02}-{d:02}"));
            }
        }
        for s in &cases {
            let expected = shape::is_date(s).then(|| {
                let d = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap();
                i64::from(chrono::Datelike::num_days_from_ce(&d)) - UNIX_EPOCH_DAYS_FROM_CE
            });
            assert_eq!(day(s), expected, "{s:?}");
        }
    }

    #[test]
    fn distinct_counts_of_sequential_numbers_are_accurate() {
        for n in [10u64, 1_000, 50_000, 400_000] {
            let (mut ints, mut floats, mut days) = (Hll::new(), Hll::new(), Hll::new());
            for i in 0..n {
                ints.insert_hash(hash_int(i128::from(i)));
                floats.insert_hash(hash_float(i as f64 * 0.5));
                days.insert_hash(hash_int(i128::from(i) * 86_400_000_000_000));
            }
            for (what, h) in [("ints", &ints), ("floats", &floats), ("instants", &days)] {
                let est = h.estimate();
                assert!(
                    (est - n as f64).abs() <= (n as f64 * 0.05).max(1.0),
                    "{what}, n = {n}: {est}"
                );
            }
        }
    }

    #[test]
    fn numbers_read_well() {
        assert_eq!(num(171_050.333), "171,050");
        assert_eq!(num(88.333_33), "88.33");
        assert_eq!(num(0.001_23), "0.00123");
        assert_eq!(num(-2.5), "-2.5");
        assert_eq!(num(-3.0), "-3");
        assert_eq!(num(0.0), "0");
        assert_eq!(pct(Some(0.0005)), "<0.1%");
        assert_eq!(pct(Some(1.0)), "100%");
        assert_eq!(group(1_234_567), "1,234,567");
    }
}
