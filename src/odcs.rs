//! ODCS v3 input: the Open Data Contract Standard, read into the same
//! contract model a `covenant: 1` document parses into, so every
//! enforcement point runs it unchanged.
//!
//! **Exact or refused.** Every ODCS rule either becomes a rule the engines
//! already enforce with the same meaning, or it comes back as an
//! [`UnenforcedRule`] naming its path and why. Nothing is dropped silently:
//! a PASS against a contract whose rules were quietly skipped would certify
//! data nobody checked. Descriptive and operational metadata — descriptions,
//! tags, physical types, servers, team, SLAs, support channels — promises
//! nothing about values and is not read.
//!
//! The mapping, rule by rule, is documented in `docs/ODCS.md`.

use std::collections::HashMap;

use indexmap::IndexMap;
use serde::Deserialize;
use serde_json::{Map, Value};

use crate::error::{CovenantError, Result};
use crate::spec::{
    Contract, ConversionNote, Field, FieldType, Model, Policy, StringFormat, UnenforcedRule,
};

/// The ODCS revisions this reader has been reviewed against, oldest first:
/// every change each one made to the standard either maps onto a rule with
/// the same meaning or is refused (`docs/ODCS.md`, "Revisions"). A v3
/// document of any other revision is still read, but its `apiVersion` is
/// itself an unenforced rule: a rule that revision introduced would
/// otherwise go unchecked without a word.
pub const VERSIONS: &[&str] = &["v3.0.0", "v3.0.1", "v3.0.2", "v3.1.0", "v3.2.0"];

/// True for a document that declares itself an ODCS contract (an
/// `apiVersion` and a `kind`, and no `covenant:` revision).
pub(crate) fn looks_like_odcs(doc: &serde_yaml::Value) -> bool {
    let Some(map) = doc.as_mapping() else {
        return false;
    };
    let has = |k: &str| map.contains_key(serde_yaml::Value::String(k.to_string()));
    has("apiVersion") && has("kind") && !has("covenant")
}

/// The result of reading one ODCS document.
pub(crate) struct Converted {
    pub contract: Contract,
    pub api_version: String,
    pub unenforced: Vec<UnenforcedRule>,
    pub notes: Vec<ConversionNote>,
}

/// Read an ODCS v3 document into a [`Contract`], collecting every rule it
/// cannot enforce. Errors only when the document is not a readable ODCS v3
/// contract at all.
pub(crate) fn convert(source: &str, origin: &str) -> Result<Converted> {
    let doc: Doc =
        serde_yaml::from_str(source).map_err(|e| parse_err(origin, format!("ODCS: {e}")))?;
    if doc.kind != "DataContract" {
        return Err(parse_err(
            origin,
            format!("ODCS kind {:?} is not DataContract", doc.kind),
        ));
    }
    if !doc.api_version.starts_with("v3.") {
        return Err(parse_err(
            origin,
            format!(
                "ODCS {} is not supported: Covenant reads ODCS {}",
                doc.api_version,
                VERSIONS.join(", ")
            ),
        ));
    }

    let mut cx = Cx::default();
    if !VERSIONS.contains(&doc.api_version.as_str()) {
        cx.unenforced(
            "apiVersion",
            format!("apiVersion {}", doc.api_version),
            format!(
                "this Covenant reads ODCS {}; a rule {} introduced would go unchecked",
                VERSIONS.join(", "),
                doc.api_version
            ),
        );
    }
    let mut models = IndexMap::new();
    for obj in &doc.schema {
        if models.contains_key(&obj.name) {
            return Err(parse_err(
                origin,
                format!("ODCS schema declares {:?} twice", obj.name),
            ));
        }
        let model = convert_object(obj, &mut cx);
        models.insert(obj.name.clone(), model);
    }

    Ok(Converted {
        contract: Contract {
            covenant: 1,
            id: scalar(&doc.id),
            version: scalar(&doc.version),
            name: doc.name.clone(),
            owner: team_name(doc.team.as_ref()),
            description: purpose(doc.description.as_ref()),
            models,
            policy: Policy::default(),
        },
        api_version: doc.api_version,
        unenforced: cx.unenforced,
        notes: cx.notes,
    })
}

// --- the ODCS document, as far as enforcement needs it ---------------------
//
// Unknown keys are ignored on purpose: ODCS carries a great deal of
// metadata that promises nothing about values. Every key that DOES carry a
// promise is read below, and anything it cannot enforce is reported.

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Doc {
    api_version: String,
    kind: String,
    id: Value,
    version: Value,
    name: Option<String>,
    description: Option<Value>,
    team: Option<Value>,
    #[serde(default)]
    schema: Vec<SchemaObject>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SchemaObject {
    name: String,
    description: Option<String>,
    #[serde(default)]
    properties: Vec<Property>,
    #[serde(default)]
    quality: Vec<Quality>,
    #[serde(default)]
    relationships: Vec<Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Property {
    name: String,
    physical_name: Option<String>,
    logical_type: Option<String>,
    #[serde(default)]
    logical_type_options: Map<String, Value>,
    required: Option<bool>,
    unique: Option<bool>,
    primary_key: Option<bool>,
    #[serde(rename = "enum")]
    enum_values: Option<Vec<EnumValue>>,
    #[serde(default)]
    quality: Vec<Quality>,
    #[serde(default)]
    relationships: Vec<Value>,
    description: Option<String>,
    deprecated: Option<bool>,
    /// v3.2: `column` (the default), `dimension` or `measure`.
    semantic_type: Option<String>,
}

impl Property {
    /// Whether the property promises anything about its values beyond its
    /// type (used when it declares no type at all).
    fn promises_anything(&self) -> bool {
        self.required == Some(true)
            || self.unique == Some(true)
            || self.primary_key == Some(true)
            || self.enum_values.is_some()
            || !self.logical_type_options.is_empty()
            || !self.quality.is_empty()
            || !self.relationships.is_empty()
    }
}

/// ODCS v3.2 enum entries are objects with a `value`; plain values are
/// accepted too.
#[derive(Deserialize)]
#[serde(untagged)]
enum EnumValue {
    Entry { value: Value },
    Plain(Value),
}

impl EnumValue {
    fn value(&self) -> &Value {
        match self {
            EnumValue::Entry { value } | EnumValue::Plain(value) => value,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Quality {
    #[serde(rename = "type")]
    ty: Option<String>,
    metric: Option<String>,
    /// Deprecated spelling of `metric`.
    rule: Option<String>,
    engine: Option<String>,
    #[serde(default)]
    arguments: Map<String, Value>,
    unit: Option<String>,
    must_be: Option<Value>,
    must_not_be: Option<Value>,
    must_be_greater_than: Option<f64>,
    must_be_greater_or_equal_to: Option<f64>,
    must_be_less_than: Option<f64>,
    must_be_less_or_equal_to: Option<f64>,
    must_be_between: Option<Vec<f64>>,
    must_not_be_between: Option<Vec<f64>>,
}

enum QualityKind<'a> {
    Library(&'a str),
    Text,
    Sql,
    Custom,
    Unknown(String),
}

impl Quality {
    fn kind(&self) -> QualityKind<'_> {
        let metric = self.metric.as_deref().or(self.rule.as_deref());
        match self.ty.as_deref() {
            Some("text") => QualityKind::Text,
            Some("sql") => QualityKind::Sql,
            Some("custom") => QualityKind::Custom,
            Some("library") | None => match metric {
                Some(m) => QualityKind::Library(m),
                None => QualityKind::Unknown("library rule without a metric".into()),
            },
            Some(other) => QualityKind::Unknown(format!("quality type {other:?}")),
        }
    }

    /// Whether the rule tolerates no offending rows at all — the only form
    /// that maps exactly onto a per-value rule. `Err` explains a rule too
    /// malformed to judge.
    fn zero_tolerance(&self) -> std::result::Result<bool, String> {
        let percent = match self.unit.as_deref().unwrap_or("rows") {
            "rows" => false,
            "percent" => true,
            other => return Err(format!("unknown unit {other:?}")),
        };
        let operators = [
            self.must_be.is_some(),
            self.must_not_be.is_some(),
            self.must_be_greater_than.is_some(),
            self.must_be_greater_or_equal_to.is_some(),
            self.must_be_less_than.is_some(),
            self.must_be_less_or_equal_to.is_some(),
            self.must_be_between.is_some(),
            self.must_not_be_between.is_some(),
        ]
        .iter()
        .filter(|&&set| set)
        .count();
        match operators {
            0 => return Err("the rule declares no operator (mustBe, mustBeLessThan, ...)".into()),
            1 => {}
            _ => return Err("the rule declares more than one operator".into()),
        }
        // A count of offending rows is a whole number, so for `rows`
        // "< t" with 0 < t <= 1 and "<= t" with 0 <= t < 1 both mean zero.
        let zero_ceiling = |hi: f64| {
            if percent {
                hi == 0.0
            } else {
                (0.0..1.0).contains(&hi)
            }
        };
        if let Some(v) = &self.must_be {
            return Ok(v.as_f64() == Some(0.0));
        }
        if let Some(t) = self.must_be_less_than {
            return Ok(!percent && t > 0.0 && t <= 1.0);
        }
        if let Some(t) = self.must_be_less_or_equal_to {
            return Ok(zero_ceiling(t));
        }
        if let Some(bounds) = &self.must_be_between {
            let [a, b] = bounds.as_slice() else {
                return Err("mustBeBetween needs exactly two numbers".into());
            };
            let (lo, hi) = (a.min(*b), a.max(*b));
            // The standard defines mustBeBetween twice: its operator table
            // gives the symbol ∈ (bounds included), and its prose equates it
            // with mustBeGreaterThan plus mustBeLessThan (bounds excluded).
            // Enforce it only where both readings mean "no offending rows".
            let included = lo <= 0.0 && zero_ceiling(hi);
            let excluded = !percent && lo < 0.0 && hi > 0.0 && hi <= 1.0;
            if included != excluded {
                let (yes, no) = if included {
                    (
                        "included, as the operator table's ∈ says",
                        "excluded, as its prose says",
                    )
                } else {
                    (
                        "excluded, as the standard's prose says",
                        "included, as its ∈ says",
                    )
                };
                return Err(format!(
                    "mustBeBetween [{lo}, {hi}] means no offending rows if its bounds are {yes}, \
                     but not if they are {no}; the standard says both, so it is not enforced. \
                     For zero tolerance, write mustBe: 0"
                ));
            }
            return Ok(included);
        }
        // mustNotBe and the greater-than / not-between forms never mean
        // "no offending rows".
        Ok(false)
    }
}

// --- conversion --------------------------------------------------------------

#[derive(Default)]
struct Cx {
    unenforced: Vec<UnenforcedRule>,
    notes: Vec<ConversionNote>,
}

impl Cx {
    fn unenforced(
        &mut self,
        path: impl Into<String>,
        rule: impl Into<String>,
        reason: impl Into<String>,
    ) {
        self.unenforced.push(UnenforcedRule {
            path: path.into(),
            rule: rule.into(),
            reason: reason.into(),
        });
    }

    fn note(&mut self, path: impl Into<String>, message: impl Into<String>) {
        self.notes.push(ConversionNote {
            path: path.into(),
            message: message.into(),
        });
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum KeyRole {
    None,
    /// The model's only primary-key property.
    Single,
    /// One part of a composite primary key.
    Part,
}

fn convert_object(obj: &SchemaObject, cx: &mut Cx) -> Model {
    let opath = format!("schema.{}", obj.name);
    for i in 0..obj.relationships.len() {
        cx.unenforced(
            format!("{opath}.relationships[{i}]"),
            "relationship",
            "references to other datasets are not checked at a single boundary",
        );
    }

    let key_parts: Vec<&str> = obj
        .properties
        .iter()
        .filter(|p| p.primary_key == Some(true))
        .map(|p| p.name.as_str())
        .collect();
    let composite = key_parts.len() > 1;
    if composite {
        cx.unenforced(
            opath.clone(),
            "primaryKey",
            format!(
                "uniqueness of the composite key ({}) is not checked yet; each part is still checked as not null",
                key_parts.join(", ")
            ),
        );
    }

    let mut fields: IndexMap<String, Field> = IndexMap::new();
    // ODCS names properties logically; the data carries physical column
    // names. Schema-level rules refer to logical names, so keep the map.
    let mut column_of: HashMap<&str, String> = HashMap::new();
    for prop in &obj.properties {
        let path = format!("{opath}.properties.{}", prop.name);
        let role = match prop.primary_key {
            Some(true) if composite => KeyRole::Part,
            Some(true) => KeyRole::Single,
            _ => KeyRole::None,
        };
        let Some(field) = convert_property(prop, &path, role, cx) else {
            continue;
        };
        let column = prop
            .physical_name
            .clone()
            .unwrap_or_else(|| prop.name.clone());
        if let Some(var) = variable_in(&column) {
            let key = if prop.physical_name.is_some() {
                "physicalName"
            } else {
                "name"
            };
            cx.unenforced(
                path,
                key,
                format!(
                    "the column name {}, so the property is not checked",
                    unresolved(var)
                ),
            );
            continue;
        }
        if fields.contains_key(&column) {
            cx.unenforced(
                path,
                "physicalName",
                format!(
                    "a second property maps to the column {column:?}; only the first is checked"
                ),
            );
            continue;
        }
        column_of.insert(prop.name.as_str(), column.clone());
        fields.insert(column, field);
    }

    for (i, q) in obj.quality.iter().enumerate() {
        schema_quality(
            q,
            &format!("{opath}.quality[{i}]"),
            &mut fields,
            &column_of,
            cx,
        );
    }

    Model {
        description: obj.description.clone(),
        // ODCS schemas are open: columns the contract does not declare
        // are not violations.
        strict: false,
        fields,
    }
}

fn convert_property(prop: &Property, path: &str, role: KeyRole, cx: &mut Cx) -> Option<Field> {
    match prop.semantic_type.as_deref() {
        // A dimension is an attribute each record carries, like a column.
        None | Some("column" | "dimension") => {}
        // A measure is computed over the records (`SUM(revenue)`): its
        // logicalType describes that result, and its rules constrain it.
        Some("measure") => {
            if prop.promises_anything() {
                cx.unenforced(
                    path,
                    "semanticType measure",
                    "a rule on a measure constrains an aggregate over the records: not checked yet",
                );
            } else {
                cx.note(
                    path,
                    "a measure: an aggregate its transformLogic computes over the records, not a value each record carries, so it is not checked as a column",
                );
            }
            return None;
        }
        Some(other) => {
            cx.unenforced(
                path,
                "semanticType",
                format!("unknown semanticType {other:?}"),
            );
            return None;
        }
    }
    let ty = match prop.logical_type.as_deref() {
        Some("string") => FieldType::String,
        Some("integer") => FieldType::Integer,
        Some("number") => FieldType::Float,
        Some("boolean") => FieldType::Boolean,
        Some("date") => FieldType::Date,
        Some("timestamp") => FieldType::Timestamp,
        Some(t @ ("time" | "object" | "array" | "map" | "vector")) => {
            cx.unenforced(
                path,
                "logicalType",
                format!("{t} values are not checked yet"),
            );
            return None;
        }
        Some(t) => {
            cx.unenforced(path, "logicalType", format!("unknown logicalType {t:?}"));
            return None;
        }
        None => {
            if prop.promises_anything() {
                cx.unenforced(
                    path,
                    "logicalType",
                    "the property declares no logicalType, so its rules cannot be checked",
                );
            }
            return None;
        }
    };

    let mut field = Field {
        ty,
        description: prop.description.clone(),
        // ODCS `required: false` (the default) means the value may be null
        // or absent; `required: true` means it may not.
        required: false,
        nullable: true,
        unique: false,
        pattern: None,
        min: None,
        max: None,
        min_length: None,
        max_length: None,
        allowed: None,
        format: None,
        deprecated: None,
    };
    if prop.required == Some(true) {
        not_null(&mut field);
    }
    if prop.unique == Some(true) {
        field.unique = true;
    }
    match role {
        KeyRole::Single => {
            if prop.required == Some(false) {
                cx.note(path, "primaryKey is checked as unique and not null, although the property says required: false");
            }
            field.unique = true;
            not_null(&mut field);
        }
        KeyRole::Part => {
            if prop.required == Some(false) {
                cx.note(path, "part of a composite primary key: checked as not null, although the property says required: false");
            }
            not_null(&mut field);
        }
        KeyRole::None => {}
    }
    if prop.deprecated == Some(true) {
        field.deprecated = Some(String::new());
    }

    if !type_options(
        &mut field,
        &prop.logical_type_options,
        &format!("{path}.logicalTypeOptions"),
        cx,
    ) {
        return None;
    }
    if let Some(values) = &prop.enum_values {
        match values
            .iter()
            .find_map(|v| v.value().as_str().and_then(variable_in))
        {
            Some(var) => cx.unenforced(
                format!("{path}.enum"),
                "enum",
                format!("a value {}", unresolved(var)),
            ),
            None => restrict_to(&mut field, values.iter().map(EnumValue::value)),
        }
    }
    for i in 0..prop.relationships.len() {
        cx.unenforced(
            format!("{path}.relationships[{i}]"),
            "relationship",
            "references to other datasets are not checked at a single boundary",
        );
    }
    for (i, q) in prop.quality.iter().enumerate() {
        property_quality(&mut field, q, &format!("{path}.quality[{i}]"), cx);
    }
    Some(field)
}

/// No nulls, and (because a missing key is a null in columnar data) no
/// missing keys: `required` + not `nullable`. An optional non-nullable
/// field would let columnar nulls through — see ARCHITECTURE.md.
fn not_null(field: &mut Field) {
    field.required = true;
    field.nullable = false;
}

/// ODCS v3.0.x spells an exclusive bound as a flag on `minimum`/`maximum`
/// (JSON Schema draft 4); v3.1 gives the bound itself. `false`, or `true`
/// with no bound to make exclusive, excludes nothing.
fn excludes_nothing(key: &str, v: &Value, opts: &Map<String, Value>) -> bool {
    let bound = if key == "exclusiveMinimum" {
        "minimum"
    } else {
        "maximum"
    };
    *v == Value::Bool(false) || (*v == Value::Bool(true) && !opts.contains_key(bound))
}

/// The value an exclusive bound excludes: the number itself (v3.1), or the
/// `minimum`/`maximum` that v3.0.x's `true` makes exclusive.
fn exclusive_bound(v: &Value, inclusive: Option<&Value>) -> Option<f64> {
    match v {
        Value::Bool(true) => inclusive.and_then(Value::as_f64),
        _ => v.as_f64(),
    }
}

/// Apply `logicalTypeOptions`. Returns false when the field cannot be
/// checked faithfully at all and must be left out.
fn type_options(field: &mut Field, opts: &Map<String, Value>, path: &str, cx: &mut Cx) -> bool {
    let bad = |cx: &mut Cx, key: &str, v: &Value| {
        cx.unenforced(
            format!("{path}.{key}"),
            key,
            format!("unreadable value {v}"),
        );
    };
    let mut string_format = None;
    for (key, v) in opts {
        let kpath = format!("{path}.{key}");
        match (field.ty, key.as_str()) {
            (_, "exclusiveMinimum" | "exclusiveMaximum") if excludes_nothing(key, v, opts) => {}
            (FieldType::String, "minLength") => match v.as_u64() {
                Some(n) => field.min_length = Some(n as usize),
                None => bad(cx, key, v),
            },
            (FieldType::String, "maxLength") => match v.as_u64() {
                Some(n) => field.max_length = Some(n as usize),
                None => bad(cx, key, v),
            },
            (FieldType::String, "pattern") => match v.as_str() {
                Some(p) => match variable_in(p) {
                    Some(var) => cx.unenforced(
                        kpath,
                        key.as_str(),
                        format!("the pattern {}", unresolved(var)),
                    ),
                    None => field.pattern = Some(p.to_string()),
                },
                None => bad(cx, key, v),
            },
            // Applied last: `uuid` changes the field's type.
            (FieldType::String, "format") => string_format = Some(v),
            (FieldType::Integer | FieldType::Float, "minimum") => match v.as_f64() {
                Some(n) => field.min = Some(field.min.map_or(n, |m| m.max(n))),
                None => bad(cx, key, v),
            },
            (FieldType::Integer | FieldType::Float, "maximum") => match v.as_f64() {
                Some(n) => field.max = Some(field.max.map_or(n, |m| m.min(n))),
                None => bad(cx, key, v),
            },
            // Integers make exclusive bounds exact: > 5 is >= 6.
            (FieldType::Integer, "exclusiveMinimum") => {
                match exclusive_bound(v, opts.get("minimum")) {
                    Some(n) => {
                        let n = n.floor() + 1.0;
                        field.min = Some(field.min.map_or(n, |m| m.max(n)));
                    }
                    None => bad(cx, key, v),
                }
            }
            (FieldType::Integer, "exclusiveMaximum") => {
                match exclusive_bound(v, opts.get("maximum")) {
                    Some(n) => {
                        let n = n.ceil() - 1.0;
                        field.max = Some(field.max.map_or(n, |m| m.min(n)));
                    }
                    None => bad(cx, key, v),
                }
            }
            (FieldType::Float, "exclusiveMinimum" | "exclusiveMaximum") => {
                cx.unenforced(
                    kpath,
                    key.as_str(),
                    "exclusive bounds on number fields are not checked yet",
                );
            }
            (FieldType::Integer | FieldType::Float, "multipleOf") => {
                cx.unenforced(kpath, key.as_str(), "multipleOf is not checked yet");
            }
            (FieldType::Integer | FieldType::Float, "format") => {
                cx.unenforced(
                    kpath,
                    key.as_str(),
                    format!("number format {v} is not checked"),
                );
            }
            // Covenant's date type IS this shape.
            (FieldType::Date, "format") if is_iso_date_format(v) => {}
            // A custom format means the values are NOT the shape the date
            // and timestamp checks accept: checking them anyway would fail
            // conforming data, so the whole field is left out.
            (FieldType::Date | FieldType::Timestamp, "format") => {
                cx.unenforced(
                    path.trim_end_matches(".logicalTypeOptions"),
                    format!("{} format", field.ty.name()),
                    format!(
                        "format {v}: Covenant checks {} values as {}, so this property is not checked",
                        field.ty.name(),
                        if field.ty == FieldType::Date { "YYYY-MM-DD" } else { "RFC 3339" }
                    ),
                );
                return false;
            }
            (FieldType::Date | FieldType::Timestamp, _) => {
                cx.unenforced(
                    kpath,
                    key.as_str(),
                    format!("{key} on {} values is not checked yet", field.ty.name()),
                );
            }
            _ => {
                cx.unenforced(
                    kpath,
                    key.as_str(),
                    format!("{key} is not checked for {} values", field.ty.name()),
                );
            }
        }
    }
    if let Some(v) = string_format {
        let kpath = format!("{path}.format");
        match v.as_str() {
            Some("email") => field.format = Some(StringFormat::Email),
            Some("uri") => field.format = Some(StringFormat::Uri),
            Some("uuid")
                if field.pattern.is_none()
                    && field.min_length.is_none()
                    && field.max_length.is_none() =>
            {
                field.ty = FieldType::Uuid;
            }
            Some("uuid") => cx.unenforced(
                kpath,
                "format",
                "format uuid together with pattern or length options is not checked yet",
            ),
            _ => cx.unenforced(kpath, "format", format!("string format {v} is not checked")),
        }
    }
    true
}

/// The first variable reference in `s`: ODCS v3.2 lets any string hold
/// `${NAME}` or `${NAME:-default}`, for the tool to resolve before it uses
/// the value. Covenant does not resolve variables yet, so a rule holding one
/// is refused rather than checked against the literal text.
fn variable_in(s: &str) -> Option<&str> {
    let mut from = 0;
    while let Some(start) = s[from..].find("${").map(|i| from + i) {
        let body = start + 2;
        let end = body + s[body..].find('}')?;
        let inner = &s[body..end];
        let name = inner.split_once(":-").map_or(inner, |(name, _)| name);
        let mut chars = name.chars();
        if chars
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            return Some(&s[start..=end]);
        }
        from = body;
    }
    None
}

fn unresolved(var: &str) -> String {
    format!("holds the variable {var}, which Covenant does not resolve yet")
}

fn is_iso_date_format(v: &Value) -> bool {
    matches!(v.as_str(), Some("yyyy-MM-dd" | "YYYY-MM-DD" | "%Y-%m-%d"))
}

/// Narrow a field's allowed set to `values` (intersecting an existing one:
/// both rules must hold). A null among the values passes this rule and
/// changes nothing else: whether the field may be null is decided by
/// `required`, a primary key or `nullValues`, which a list cannot relax.
fn restrict_to<'a>(field: &mut Field, values: impl Iterator<Item = &'a Value>) {
    let set: Vec<Value> = values.filter(|v| !v.is_null()).cloned().collect();
    field.allowed = Some(match field.allowed.take() {
        Some(existing) => existing
            .into_iter()
            .filter(|e| set.iter().any(|s| same_value(e, s)))
            .collect(),
        None => set,
    });
}

/// JSON equality with numbers compared by value (1 == 1.0).
fn same_value(a: &Value, b: &Value) -> bool {
    match (a.as_f64(), b.as_f64()) {
        (Some(x), Some(y)) => x == y,
        _ => a == b,
    }
}

fn property_quality(field: &mut Field, q: &Quality, path: &str, cx: &mut Cx) {
    let metric = match q.kind() {
        QualityKind::Library(m) => m,
        other => return unsupported_quality(other, q, path, cx),
    };
    let label = format!("library {metric}");
    match q.zero_tolerance() {
        Err(why) => return cx.unenforced(path, label, why),
        Ok(false) => {
            return cx.unenforced(
                path,
                label,
                "threshold rules are not enforced yet: only zero tolerance (mustBe: 0) maps onto a per-value rule",
            )
        }
        Ok(true) => {}
    }
    match metric {
        "nullValues" => not_null(field),
        "duplicateValues" => field.unique = true,
        "invalidValues" => invalid_values(field, &q.arguments, path, &label, cx),
        "missingValues" => missing_values(field, &q.arguments, path, &label, cx),
        "rowCount" => cx.unenforced(
            path,
            label,
            "rowCount is a dataset-level rule: not enforced yet",
        ),
        other => cx.unenforced(path, label, format!("unknown metric {other:?}")),
    }
}

fn invalid_values(
    field: &mut Field,
    args: &Map<String, Value>,
    path: &str,
    label: &str,
    cx: &mut Cx,
) {
    let var = match (args.get("validValues"), args.get("pattern")) {
        (Some(Value::Array(values)), None) => values
            .iter()
            .find_map(|v| v.as_str().and_then(variable_in))
            .map(|var| format!("a valid value {}", unresolved(var))),
        (None, Some(Value::String(p))) => {
            variable_in(p).map(|var| format!("the pattern {}", unresolved(var)))
        }
        _ => None,
    };
    if let Some(why) = var {
        return cx.unenforced(path, label, why);
    }
    match (args.get("validValues"), args.get("pattern")) {
        (Some(Value::Array(values)), None) => restrict_to(field, values.iter()),
        (None, Some(Value::String(p))) => match &field.pattern {
            Some(existing) if existing != p => cx.unenforced(
                path,
                label,
                "a second, different pattern on the same property is not checked yet",
            ),
            _ => field.pattern = Some(p.clone()),
        },
        (Some(_), Some(_)) => cx.unenforced(
            path,
            label,
            "validValues together with pattern is not checked yet",
        ),
        _ => cx.unenforced(
            path,
            label,
            "invalidValues needs arguments.validValues (a list) or arguments.pattern",
        ),
    }
}

fn missing_values(
    field: &mut Field,
    args: &Map<String, Value>,
    path: &str,
    label: &str,
    cx: &mut Cx,
) {
    let Some(Value::Array(values)) = args.get("missingValues") else {
        return cx.unenforced(
            path,
            label,
            "missingValues needs arguments.missingValues (a list)",
        );
    };
    let checkable = values.iter().all(|v| v.is_null() || v.as_str() == Some(""));
    if !checkable {
        return cx.unenforced(
            path,
            label,
            "only null and \"\" can be enforced as missing values yet",
        );
    }
    if values.iter().any(Value::is_null) {
        not_null(field);
    }
    if field.ty == FieldType::String && values.iter().any(|v| v.as_str() == Some("")) {
        field.min_length = Some(field.min_length.unwrap_or(0).max(1));
    }
}

fn schema_quality(
    q: &Quality,
    path: &str,
    fields: &mut IndexMap<String, Field>,
    column_of: &HashMap<&str, String>,
    cx: &mut Cx,
) {
    let metric = match q.kind() {
        QualityKind::Library(m) => m,
        other => return unsupported_quality(other, q, path, cx),
    };
    let label = format!("library {metric}");
    match metric {
        "rowCount" => cx.unenforced(
            path,
            label,
            "rowCount is a dataset-level rule: not enforced yet",
        ),
        "duplicateValues" => {
            match q.zero_tolerance() {
                Err(why) => return cx.unenforced(path, label, why),
                Ok(false) => {
                    return cx.unenforced(path, label, "threshold rules are not enforced yet")
                }
                Ok(true) => {}
            }
            let names: Vec<&str> = match q.arguments.get("properties") {
                Some(Value::Array(v)) => v.iter().filter_map(Value::as_str).collect(),
                _ => Vec::new(),
            };
            match names.as_slice() {
                [one] => match column_of.get(one).and_then(|c| fields.get_mut(c)) {
                    Some(field) => field.unique = true,
                    None => cx.unenforced(
                        path,
                        label,
                        format!("property {one:?} is not checked, so neither is its uniqueness"),
                    ),
                },
                [] => cx.unenforced(
                    path,
                    label,
                    "duplicateValues at schema level needs arguments.properties",
                ),
                many => cx.unenforced(
                    path,
                    label,
                    format!("uniqueness across ({}) is not checked yet", many.join(", ")),
                ),
            }
        }
        other => cx.unenforced(
            path,
            label,
            format!("{other} at schema level is not checked yet"),
        ),
    }
}

fn unsupported_quality(kind: QualityKind<'_>, q: &Quality, path: &str, cx: &mut Cx) {
    match kind {
        QualityKind::Text => cx.unenforced(path, "text", "a prose rule: not machine-checkable"),
        QualityKind::Sql => {
            cx.unenforced(path, "sql", "SQL rules need a SQL engine: not supported")
        }
        QualityKind::Custom => cx.unenforced(
            path,
            "custom",
            format!(
                "a custom rule for engine {:?}: not supported",
                q.engine.as_deref().unwrap_or("unnamed")
            ),
        ),
        QualityKind::Unknown(what) => {
            cx.unenforced(path, "quality", format!("{what} is not understood"))
        }
        QualityKind::Library(_) => unreachable!("library rules are handled by the caller"),
    }
}

// --- document-level metadata --------------------------------------------------

fn scalar(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// The owner reports name: the team's name (v3.1+), or the first member of
/// the deprecated member-list form.
fn team_name(team: Option<&Value>) -> Option<String> {
    match team? {
        Value::Object(t) => t.get("name").and_then(Value::as_str).map(str::to_string),
        Value::Array(members) => members.first().and_then(|m| {
            m.get("username")
                .or_else(|| m.get("name"))
                .and_then(Value::as_str)
                .map(str::to_string)
        }),
        _ => None,
    }
}

/// ODCS v3's top-level description is an object; its `purpose` is the
/// closest thing to a one-line description.
fn purpose(description: Option<&Value>) -> Option<String> {
    match description? {
        Value::String(s) => Some(s.clone()),
        Value::Object(d) => d.get("purpose").and_then(Value::as_str).map(str::to_string),
        _ => None,
    }
}

fn parse_err(origin: &str, message: String) -> CovenantError {
    CovenantError::ContractParse {
        path: origin.to_string(),
        message,
    }
}
