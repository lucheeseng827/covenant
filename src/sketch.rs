//! Mergeable sketches behind profiles: HyperLogLog for distinct counts and a
//! log-bucketed histogram for quantiles.
//!
//! Both are deterministic and blind to order — the same values give the same
//! sketch, bit for bit, however they were batched — and both serialize to
//! plain JSON, so a sketch written by one run merges with one written by
//! another run, on another machine, by another release. That is also why [`hash64`] exists instead
//! of `std`'s hashers: those promise stability across neither platforms nor
//! releases, and a persisted sketch needs both.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A stable 64-bit hash of `bytes`: word-at-a-time mixing, then murmur3's
/// 64-bit finalizer for avalanche. Not cryptographic; uniform enough for
/// HyperLogLog, and fixed forever — changing it would invalidate every
/// profile ever written.
pub fn hash64(bytes: &[u8]) -> u64 {
    hash64_and_scalars(bytes).0
}

/// [`hash64`] of UTF-8 bytes together with their Unicode scalar count — the
/// length `min_length` measures — in one pass over their words.
#[inline(always)]
pub fn hash64_and_scalars(bytes: &[u8]) -> (u64, u64) {
    const HIGH: u64 = 0x8080_8080_8080_8080;
    // A continuation byte is 10xxxxxx: top bit set, the next one clear. An
    // all-ASCII word has none, and skips the population count.
    let continuations = |w: u64| {
        if w & HIGH == 0 {
            0
        } else {
            u64::from((w & HIGH & !((w << 1) & HIGH)).count_ones())
        }
    };
    let mut h: u64 = 0x9E37_79B9_7F4A_7C15 ^ (bytes.len() as u64);
    let mut cont = 0;
    let mut words = bytes.chunks_exact(8);
    for w in &mut words {
        let w = u64::from_le_bytes([w[0], w[1], w[2], w[3], w[4], w[5], w[6], w[7]]);
        cont += continuations(w);
        h = (h ^ w).wrapping_mul(0xFF51_AFD7_ED55_8CCD).rotate_left(29);
    }
    let rest = words.remainder();
    if !rest.is_empty() {
        let w = tail_word(rest);
        cont += continuations(w);
        h = (h ^ w).wrapping_mul(0xC4CE_B9FE_1A85_EC53).rotate_left(31);
    }
    (mix64(h), bytes.len() as u64 - cont)
}

/// Up to seven bytes as a zero-padded little-endian word, assembled from 4-,
/// 2- and 1-byte pieces rather than copied (no memcpy call).
#[inline(always)]
pub fn tail_word(rest: &[u8]) -> u64 {
    let b = |i: usize| u64::from(rest[i]);
    // Constant indices per length, so no bounds check survives.
    match *rest {
        [] => 0,
        [a] => u64::from(a),
        [a, c] => u64::from(u16::from_le_bytes([a, c])),
        [a, c, d] => u64::from(u16::from_le_bytes([a, c])) | u64::from(d) << 16,
        [a, c, d, e] => u64::from(u32::from_le_bytes([a, c, d, e])),
        [a, c, d, e, f] => u64::from(u32::from_le_bytes([a, c, d, e])) | u64::from(f) << 32,
        [a, c, d, e, f, g] => {
            u64::from(u32::from_le_bytes([a, c, d, e]))
                | u64::from(u16::from_le_bytes([f, g])) << 32
        }
        [a, c, d, e, f, g, h] => {
            u64::from(u32::from_le_bytes([a, c, d, e]))
                | u64::from(u16::from_le_bytes([f, g])) << 32
                | u64::from(h) << 48
        }
        _ => (0..rest.len().min(8)).fold(0, |w, i| w | b(i) << (8 * i)),
    }
}

/// murmur3's 64-bit finalizer: a bijection in which every input bit affects
/// every output bit — on its own, a perfect hash of a 64-bit value. Pinned
/// like [`hash64`].
#[inline(always)]
pub fn mix64(mut k: u64) -> u64 {
    k ^= k >> 33;
    k = k.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
    k ^= k >> 33;
    k = k.wrapping_mul(0xC4CE_B9FE_1A85_EC53);
    k ^= k >> 33;
    k
}

// --- HyperLogLog -----------------------------------------------------------------

/// Register-index bits: 4,096 registers, a standard error of about 1.6 %.
const HLL_P: u32 = 12;
const HLL_M: usize = 1 << HLL_P;
/// Hash bits left for the rank once the index is taken.
const HLL_Q: u32 = 64 - HLL_P;

/// A HyperLogLog distinct-count sketch. Merging takes the register-wise
/// maximum, so a sketch of the union costs nothing extra and loses nothing.
/// The estimate is Ertl's improved estimator ("New cardinality estimation
/// algorithms for HyperLogLog sketches", 2017): no bias-correction tables,
/// and no switch-over between small and large ranges.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hll {
    registers: Vec<u8>,
}

impl Default for Hll {
    fn default() -> Self {
        Hll::new()
    }
}

impl Hll {
    pub fn new() -> Self {
        Hll {
            registers: vec![0; HLL_M],
        }
    }

    /// Add one value, given its hash ([`hash64`] or [`mix64`]).
    #[inline(always)]
    pub fn insert_hash(&mut self, h: u64) {
        let idx = (h >> HLL_Q) as usize;
        let w = h << HLL_P;
        let rank = (if w == 0 {
            HLL_Q + 1
        } else {
            w.leading_zeros() + 1
        }) as u8;
        if self.registers[idx] < rank {
            self.registers[idx] = rank;
        }
    }

    /// Fold `other` in: afterwards this sketch describes the union.
    pub fn merge(&mut self, other: &Hll) {
        for (a, b) in self.registers.iter_mut().zip(&other.registers) {
            *a = (*a).max(*b);
        }
    }

    /// Estimated number of distinct values inserted.
    pub fn estimate(&self) -> f64 {
        let m = HLL_M as f64;
        let q = HLL_Q as usize;
        let mut c = vec![0u32; q + 2];
        for &r in &self.registers {
            c[r as usize] += 1;
        }
        let mut z = m * tau(1.0 - f64::from(c[q + 1]) / m);
        for k in (1..=q).rev() {
            z = 0.5 * (z + f64::from(c[k]));
        }
        z += m * sigma(f64::from(c[0]) / m);
        let alpha_inf = 1.0 / (2.0 * std::f64::consts::LN_2);
        alpha_inf * m * m / z
    }
}

/// Ertl's σ: the correction for registers still at zero.
fn sigma(x: f64) -> f64 {
    if x == 1.0 {
        return f64::INFINITY;
    }
    let mut x = x;
    let mut y = 1.0;
    let mut z = x;
    loop {
        x *= x;
        let prev = z;
        z += x * y;
        y += y;
        if z == prev {
            return z;
        }
    }
}

/// Ertl's τ: the correction for registers at the maximum rank.
fn tau(x: f64) -> f64 {
    if x == 0.0 || x == 1.0 {
        return 0.0;
    }
    let mut x = x;
    let mut y = 1.0;
    let mut z = 1.0 - x;
    loop {
        x = x.sqrt();
        let prev = z;
        y *= 0.5;
        z -= (1.0 - x).powi(2) * y;
        if z == prev {
            return z / 3.0;
        }
    }
}

/// On disk: `{"p": 12, "registers": "<base64>"}`.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct HllRepr {
    p: u32,
    registers: String,
}

impl Serialize for Hll {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        HllRepr {
            p: HLL_P,
            registers: base64_encode(&self.registers),
        }
        .serialize(s)
    }
}

impl<'de> Deserialize<'de> for Hll {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let repr = HllRepr::deserialize(d)?;
        if repr.p != HLL_P {
            return Err(D::Error::custom(format!(
                "HyperLogLog precision {} is not supported (this release reads {HLL_P})",
                repr.p
            )));
        }
        let registers = base64_decode(&repr.registers)
            .ok_or_else(|| D::Error::custom("HyperLogLog registers are not valid base64"))?;
        if registers.len() != HLL_M || registers.iter().any(|&r| u32::from(r) > HLL_Q + 1) {
            return Err(D::Error::custom("HyperLogLog registers are malformed"));
        }
        Ok(Hll { registers })
    }
}

// --- Quantiles -----------------------------------------------------------------------

/// Buckets kept per sign before the smallest magnitudes fold together: 64
/// octaves, about 19 decades, at full resolution.
const Q_MAX_BUCKETS: usize = 4_096;
/// One past the last bucket a finite value can land in.
const Q_END: u32 = 0x7FF << 6;

/// A quantile sketch over finite `f64` values: a histogram with logarithmic
/// buckets, in the manner of DDSketch. A value's bucket is its float's top
/// 18 bits (sign aside: exponent and six mantissa bits), so each power of two
/// splits into 64 buckets, finding one is a shift, and a quantile read from a
/// bucket's midpoint is within 1/128 (0.78 %) of the true value.
///
/// Unlike rank-based sketches (KLL, t-digest), the result does not depend on
/// the order values arrive in: merging two sketches gives exactly the sketch
/// of both streams, and the same values give the same sketch however they
/// were batched or serialized. Past [`Q_MAX_BUCKETS`] buckets per sign the
/// smallest magnitudes fold into the lowest kept bucket, so accuracy is kept
/// where most data lives and memory stays bounded.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Quantiles {
    neg: Store,
    zero: u64,
    pos: Store,
}

/// Dense counts for a run of buckets: `counts[j]` is bucket `offset + j`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct Store {
    offset: u32,
    counts: Vec<u64>,
}

impl Store {
    fn total(&self) -> u64 {
        self.counts.iter().sum()
    }

    #[inline(always)]
    fn add(&mut self, i: u32, n: u64) {
        let j = i.wrapping_sub(self.offset) as usize;
        if j < self.counts.len() {
            self.counts[j] += n;
        } else {
            self.add_outside(i, n);
        }
    }

    /// [`Store::add`] for a bucket outside the current run: grow, or fold.
    #[inline(never)]
    fn add_outside(&mut self, i: u32, n: u64) {
        if self.counts.is_empty() {
            self.offset = i;
            self.counts.push(n);
            return;
        }
        if i < self.offset {
            let grow = (self.offset - i) as usize;
            if self.counts.len() + grow > Q_MAX_BUCKETS {
                // Below the kept range once bounded: fold into the lowest kept
                // bucket, unless there is still room to grow part of the way.
                let room = Q_MAX_BUCKETS - self.counts.len();
                if room > 0 {
                    self.counts.splice(0..0, std::iter::repeat_n(0, room));
                    self.offset -= room as u32;
                }
                self.counts[0] += n;
                return;
            }
            self.counts.splice(0..0, std::iter::repeat_n(0, grow));
            self.offset = i;
        }
        let j = (i - self.offset) as usize;
        if j >= self.counts.len() {
            self.counts.resize(j + 1, 0);
        }
        self.counts[j] += n;
        if self.counts.len() > Q_MAX_BUCKETS {
            let excess = self.counts.len() - Q_MAX_BUCKETS;
            let folded: u64 = self.counts.drain(..excess).sum();
            self.counts[0] += folded;
            self.offset += excess as u32;
        }
    }

    /// Occupied buckets, ascending, as (bucket, count).
    fn buckets(&self) -> impl DoubleEndedIterator<Item = (u32, u64)> + '_ {
        self.counts
            .iter()
            .enumerate()
            .filter(|(_, &n)| n > 0)
            .map(|(j, &n)| (self.offset + j as u32, n))
    }
}

/// A finite non-zero value's bucket (by magnitude).
fn bucket(x: f64) -> u32 {
    (x.abs().to_bits() >> 46) as u32
}

/// A bucket's midpoint: the value a quantile in it reads as.
fn bucket_mid(i: u32) -> f64 {
    let lo = f64::from_bits(u64::from(i) << 46);
    let hi = f64::from_bits(u64::from(i + 1) << 46);
    if hi.is_finite() {
        lo + (hi - lo) / 2.0
    } else {
        lo
    }
}

impl Quantiles {
    pub fn new() -> Self {
        Quantiles::default()
    }

    /// Values counted.
    pub fn count(&self) -> u64 {
        self.neg.total() + self.zero + self.pos.total()
    }

    /// Count one finite value. Non-finite values are ignored: they have no
    /// place on the number line a quantile lives on.
    #[inline(always)]
    pub fn insert(&mut self, x: f64) {
        if !x.is_finite() {
            return;
        }
        if x == 0.0 {
            self.zero += 1;
        } else if x > 0.0 {
            self.pos.add(bucket(x), 1);
        } else {
            self.neg.add(bucket(x), 1);
        }
    }

    /// Fold `other` in: afterwards this sketch describes both streams —
    /// exactly as if one sketch had seen them all.
    pub fn merge(&mut self, other: &Quantiles) {
        for (i, n) in other.neg.buckets() {
            self.neg.add(i, n);
        }
        self.zero += other.zero;
        for (i, n) in other.pos.buckets() {
            self.pos.add(i, n);
        }
    }

    /// The value at normalized rank `q` (0 = smallest, 1 = largest).
    pub fn quantile(&self, q: f64) -> Option<f64> {
        let total = self.count();
        if total == 0 {
            return None;
        }
        let target = (q.clamp(0.0, 1.0) * total as f64).ceil().max(1.0) as u64;
        let mut seen = 0u64;
        for (i, n) in self.neg.buckets().rev() {
            seen += n;
            if seen >= target {
                return Some(-bucket_mid(i));
            }
        }
        seen += self.zero;
        if seen >= target {
            return Some(0.0);
        }
        for (i, n) in self.pos.buckets() {
            seen += n;
            if seen >= target {
                return Some(bucket_mid(i));
            }
        }
        self.pos.buckets().last().map(|(i, _)| bucket_mid(i))
    }

    /// Estimated fraction of values `<= x`, at bucket resolution: values in
    /// `x`'s own bucket count as `<= x`.
    pub fn cdf(&self, x: f64) -> f64 {
        let total = self.count();
        if total == 0 {
            return 0.0;
        }
        let below = if x.is_nan() {
            0
        } else if x < 0.0 {
            let b = if x.is_finite() { bucket(x) } else { u32::MAX };
            self.neg
                .buckets()
                .filter(|(i, _)| *i >= b)
                .map(|(_, n)| n)
                .sum()
        } else {
            let pos = if x == 0.0 {
                0
            } else {
                let b = if x.is_finite() { bucket(x) } else { u32::MAX };
                self.pos
                    .buckets()
                    .filter(|(i, _)| *i <= b)
                    .map(|(_, n)| n)
                    .sum()
            };
            self.neg.total() + self.zero + pos
        };
        below as f64 / total as f64
    }
}

/// On disk: `{"neg": [[bucket, count], …], "zero": n, "pos": [[bucket, count], …]}`,
/// occupied buckets only; empty parts are left out.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct QuantilesRepr {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    neg: Vec<(u32, u64)>,
    #[serde(default, skip_serializing_if = "is_zero")]
    zero: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pos: Vec<(u32, u64)>,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

impl Serialize for Quantiles {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        QuantilesRepr {
            neg: self.neg.buckets().collect(),
            zero: self.zero,
            pos: self.pos.buckets().collect(),
        }
        .serialize(s)
    }
}

impl<'de> Deserialize<'de> for Quantiles {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let repr = QuantilesRepr::deserialize(d)?;
        let mut q = Quantiles {
            zero: repr.zero,
            ..Quantiles::default()
        };
        for (store, pairs) in [(&mut q.neg, &repr.neg), (&mut q.pos, &repr.pos)] {
            if pairs.len() > Q_MAX_BUCKETS || !pairs.windows(2).all(|w| w[0].0 < w[1].0) {
                return Err(D::Error::custom(
                    "quantile buckets must be distinct and ascending",
                ));
            }
            for &(i, n) in pairs {
                if i >= Q_END || n == 0 {
                    return Err(D::Error::custom(format!(
                        "quantile bucket {i} with count {n} cannot come from a finite value"
                    )));
                }
                store.add(i, n);
            }
        }
        Ok(q)
    }
}

// --- base64 (RFC 4648, standard alphabet, padded) ------------------------------------

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(B64[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let s = s.as_bytes();
    if s.len() % 4 != 0 {
        return None;
    }
    let value = |c: u8| B64.iter().position(|&b| b == c).map(|p| p as u32);
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    for (i, quad) in s.chunks(4).enumerate() {
        let last = i == s.len() / 4 - 1;
        let pad = quad.iter().rev().take_while(|&&c| c == b'=').count();
        if pad > 2 || (pad > 0 && !last) {
            return None;
        }
        let mut n = 0u32;
        for &c in &quad[..4 - pad] {
            n = (n << 6) | value(c)?;
        }
        n <<= 6 * pad as u32;
        let bytes = [(n >> 16) as u8, (n >> 8) as u8, n as u8];
        out.extend_from_slice(&bytes[..3 - pad]);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_round_trips_every_length() {
        for len in 0..40 {
            let bytes: Vec<u8> = (0..len).map(|i| (i * 37 + 11) as u8).collect();
            let enc = base64_encode(&bytes);
            assert_eq!(base64_decode(&enc).as_deref(), Some(&bytes[..]), "{enc}");
        }
        assert_eq!(base64_encode(b"Man"), "TWFu");
        assert_eq!(base64_encode(b"Ma"), "TWE=");
        assert_eq!(base64_decode("TWE"), None);
        assert_eq!(base64_decode("T=E="), None);
    }

    #[test]
    fn hll_estimates_within_its_error_across_ranges() {
        for n in [0u64, 1, 10, 1_000, 20_000, 300_000] {
            let mut h = Hll::new();
            for i in 0..n {
                h.insert_hash(hash64(&i.to_le_bytes()));
            }
            let est = h.estimate();
            let tolerance = (n as f64 * 0.05).max(1.0);
            assert!(
                (est - n as f64).abs() <= tolerance,
                "n = {n}: estimate {est}"
            );
        }
    }

    #[test]
    fn hll_merge_is_the_union() {
        let (mut a, mut b, mut all) = (Hll::new(), Hll::new(), Hll::new());
        for i in 0..50_000u64 {
            let h = hash64(&i.to_le_bytes());
            if i % 2 == 0 {
                a.insert_hash(h)
            } else {
                b.insert_hash(h)
            }
            all.insert_hash(h);
        }
        // Overlap: the union of a sketch with itself is itself.
        let mut twice = a.clone();
        twice.merge(&a);
        assert_eq!(twice, a);
        a.merge(&b);
        assert_eq!(a, all);
    }

    #[test]
    fn quantiles_are_within_their_relative_error() {
        let mut q = Quantiles::new();
        let n = 100_000u64;
        for i in 1..=n {
            q.insert(((i * 7_919) % n + 1) as f64);
        }
        assert_eq!(q.count(), n);
        for p in [0.01, 0.1, 0.25, 0.5, 0.75, 0.9, 0.99] {
            let got = q.quantile(p).unwrap();
            let exact = (p * n as f64).ceil();
            assert!(
                (got - exact).abs() <= exact / 128.0 + 1.0,
                "p = {p}: {got} vs {exact}"
            );
            assert!((q.cdf(got) - p).abs() < 0.01, "cdf at p = {p}");
        }
    }

    #[test]
    fn quantiles_order_and_merge_do_not_matter() {
        let values: Vec<f64> = (0..20_000)
            .map(|i| ((i * 104_729) % 20_011) as f64 * 0.37 - 1_500.0)
            .collect();
        let mut forward = Quantiles::new();
        values.iter().for_each(|&x| forward.insert(x));
        let mut backward = Quantiles::new();
        values.iter().rev().for_each(|&x| backward.insert(x));
        assert_eq!(forward, backward);
        let (mut a, mut b) = (Quantiles::new(), Quantiles::new());
        values[..7_000].iter().for_each(|&x| a.insert(x));
        values[7_000..].iter().for_each(|&x| b.insert(x));
        a.merge(&b);
        assert_eq!(a, forward);
        // Negative, zero and positive values all rank in order.
        let median = forward.quantile(0.5).unwrap();
        let exact = {
            let mut v = values.clone();
            v.sort_by(f64::total_cmp);
            v[9_999]
        };
        assert!(
            (median - exact).abs() <= exact.abs() / 128.0 + 0.5,
            "{median} vs {exact}"
        );
    }

    #[test]
    fn quantiles_stay_bounded_and_keep_the_large_values() {
        let mut q = Quantiles::new();
        // Magnitudes from 1e-300 to 1e300: far more octaves than are kept.
        for e in -300..=300 {
            q.insert(10f64.powi(e));
        }
        assert!(q.pos.counts.len() <= Q_MAX_BUCKETS);
        assert_eq!(q.count(), 601);
        let top = q.quantile(1.0).unwrap();
        assert!((top / 1e300 - 1.0).abs() < 1.0 / 128.0, "{top}");
        let mut reversed = Quantiles::new();
        for e in (-300..=300).rev() {
            reversed.insert(10f64.powi(e));
        }
        assert_eq!(q, reversed);
    }

    #[test]
    fn sketches_are_deterministic_and_round_trip_through_json() {
        let build = || {
            let (mut h, mut q) = (Hll::new(), Quantiles::new());
            for i in 0..10_000u64 {
                h.insert_hash(hash64(&i.to_le_bytes()));
                q.insert((i % 977) as f64 * 0.5 - 100.0);
            }
            (h, q)
        };
        let (h1, q1) = build();
        let (h2, q2) = build();
        assert_eq!((&h1, &q1), (&h2, &q2));
        let h_back: Hll = serde_json::from_str(&serde_json::to_string(&h1).unwrap()).unwrap();
        let q_back: Quantiles = serde_json::from_str(&serde_json::to_string(&q1).unwrap()).unwrap();
        assert_eq!(h_back, h1);
        assert_eq!(q_back, q1);
    }

    #[test]
    fn malformed_sketches_are_refused() {
        let unsorted = r#"{"pos":[[70000,1],[69000,1]]}"#;
        assert!(serde_json::from_str::<Quantiles>(unsorted).is_err());
        let infinite = r#"{"pos":[[131008,1]]}"#;
        assert!(serde_json::from_str::<Quantiles>(infinite).is_err());
        let empty_bucket = r#"{"neg":[[70000,0]]}"#;
        assert!(serde_json::from_str::<Quantiles>(empty_bucket).is_err());
        let bad_p = r#"{"p":10,"registers":""}"#;
        assert!(serde_json::from_str::<Hll>(bad_p).is_err());
        let short = r#"{"p":12,"registers":"AAAA"}"#;
        assert!(serde_json::from_str::<Hll>(short).is_err());
    }

    #[test]
    fn the_tail_is_the_zero_padded_word() {
        // The same value the straightforward padded copy gives, every length.
        let reference = |bytes: &[u8]| -> u64 {
            let mut h: u64 = 0x9E37_79B9_7F4A_7C15 ^ (bytes.len() as u64);
            let mut words = bytes.chunks_exact(8);
            for w in &mut words {
                let w = u64::from_le_bytes(w.try_into().unwrap());
                h = (h ^ w).wrapping_mul(0xFF51_AFD7_ED55_8CCD).rotate_left(29);
            }
            let rest = words.remainder();
            if !rest.is_empty() {
                let mut buf = [0u8; 8];
                buf[..rest.len()].copy_from_slice(rest);
                h = (h ^ u64::from_le_bytes(buf))
                    .wrapping_mul(0xC4CE_B9FE_1A85_EC53)
                    .rotate_left(31);
            }
            mix64(h)
        };
        for len in 0..40 {
            let bytes: Vec<u8> = (0..len).map(|i| (i * 131 + 7) as u8).collect();
            assert_eq!(hash64(&bytes), reference(&bytes), "length {len}");
        }
    }

    #[test]
    fn scalars_are_counted_as_chars_are() {
        for s in [
            "",
            "a",
            "abcdefg",
            "abcdefgh",
            "ünïcödé",
            "日本語テキスト",
            "🦀🦀 rust 🦀",
            "x".repeat(33).as_str(),
        ] {
            let (h, n) = hash64_and_scalars(s.as_bytes());
            assert_eq!(n, s.chars().count() as u64, "{s:?}");
            assert_eq!(h, hash64(s.as_bytes()));
        }
    }

    #[test]
    fn the_hash_is_pinned() {
        // Persisted sketches depend on these exact values: a change here
        // silently corrupts every stored profile's distinct counts.
        assert_ne!(hash64(b"a"), hash64(b"a\0"));
        assert_eq!(hash64(b""), 0x9ca0_66f1_a4ab_2eea);
        assert_eq!(hash64(b"covenant"), 0xdc8b_7fd1_d8b2_cd77);
        assert_eq!(hash64(&42i128.to_le_bytes()), 0x3ca3_a56d_ce55_bbac);
        assert_eq!(mix64(0), 0);
        assert_eq!(mix64(0x2545_F491_4F6C_DD1D ^ 42), 0x6880_cd9e_bdbd_6eb6);
    }
}
