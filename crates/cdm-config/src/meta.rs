//! Per-field property metadata (`CFG-002`) and the traits that carry it.
//!
//! Every fact about a configuration property — its canonical name, its legacy `spark.cdm.*`
//! alias, its type, its default, its unit, whether it is a secret, its documentation and its
//! stability — is written exactly once, in the [`cdm_properties!`](crate::cdm_properties) macro
//! invocation that also defines the Rust field. This module holds the vocabulary those
//! invocations speak in and the machinery that turns them into a [`PropertyMeta`] row.
//!
//! # Why the default lives in the struct and not in the metadata
//!
//! A property's default is the value in `impl Default`, which the macro generates from the
//! `= <expr>` in the field declaration. [`PropertyMeta::default_value`] is *derived* from that
//! instance by serialising it, so the documented default and the value a run actually gets can
//! never disagree. That is the whole point of `CFG-001`.

use std::fmt;

use serde::Serialize;
use serde_json::Value;

/// How stable a property is (`CFG-002`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Stability {
    /// Covered by the compatibility promise; will not change meaning within a major version.
    Stable,
    /// May change or be withdrawn in a minor release.
    Experimental,
    /// Accepted for backwards compatibility; will be removed in the next major release.
    Deprecated,
}

impl Stability {
    /// The stable lowercase string form used in generated documentation and JSON.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Experimental => "experimental",
            Self::Deprecated => "deprecated",
        }
    }
}

impl fmt::Display for Stability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The kind of value a property accepts.
///
/// The kind is what lets an untyped source — a Java `.properties` file, a `CDM__*` environment
/// variable, a `--set key=value` override — be coerced into the JSON value the typed model
/// deserialises from, and it is what Tier-1 validation checks a raw value against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PropertyKind {
    /// `true` / `false`.
    Bool,
    /// A whole number.
    Integer,
    /// A 128-bit whole number, carried as a decimal string because JSON numbers cannot hold it.
    BigInteger,
    /// A number that may have a fractional part.
    Float,
    /// Free text.
    String,
    /// A filesystem path.
    Path,
    /// A credential. Never rendered; supports `env:`/`file:`/`exec:` indirection (`CFG-012`).
    Secret,
    /// A comma-separated list. An empty value is invalid (`CFG-027`).
    List,
    /// A humantime duration such as `30s`, `5m`, `100ms`.
    Duration,
    /// One of a closed set of values, matched case-insensitively.
    Enum(&'static [&'static str]),
    /// A `host:port` socket address.
    Socket,
    /// A UUID.
    Uuid,
    /// A regular expression.
    Regex,
    /// An IANA time-zone name.
    TimeZone,
    /// An absolute URL.
    Url,
}

impl PropertyKind {
    /// The name used in generated documentation and in diagnostics.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Bool => "bool",
            Self::Integer => "integer",
            Self::BigInteger => "bigint",
            Self::Float => "float",
            Self::String => "string",
            Self::Path => "path",
            Self::Secret => "secret",
            Self::List => "list",
            Self::Duration => "duration",
            Self::Enum(_) => "enum",
            Self::Socket => "socket",
            Self::Uuid => "uuid",
            Self::Regex => "regex",
            Self::TimeZone => "timezone",
            Self::Url => "url",
        }
    }

    /// Coerces a raw string from an untyped source into the JSON value the model expects.
    ///
    /// Returns the reason the value is unacceptable rather than a typed error, because every
    /// caller turns it straight into a [`Diagnostic`](cdm_core::Diagnostic) detail
    /// string (`CFG-021`).
    ///
    /// ```
    /// use cdm_config::meta::PropertyKind;
    ///
    /// assert_eq!(PropertyKind::Integer.coerce("42").unwrap(), serde_json::json!(42));
    /// assert_eq!(
    ///     PropertyKind::List.coerce("a, b").unwrap(),
    ///     serde_json::json!(["a", "b"])
    /// );
    /// assert!(PropertyKind::List.coerce("").is_err());
    /// ```
    pub fn coerce(self, raw: &str) -> Result<Value, String> {
        let trimmed = raw.trim();
        match self {
            Self::Bool => match trimmed.to_ascii_lowercase().as_str() {
                "true" | "yes" | "on" | "1" => Ok(Value::Bool(true)),
                "false" | "no" | "off" | "0" => Ok(Value::Bool(false)),
                _ => Err(format!("expected a boolean, found `{trimmed}`")),
            },
            Self::Integer => trimmed
                .parse::<i64>()
                .map(|n| Value::Number(n.into()))
                .map_err(|_| format!("expected a whole number, found `{trimmed}`")),
            Self::BigInteger => trimmed
                .parse::<i128>()
                .map(|n| Value::String(n.to_string()))
                .map_err(|_| format!("expected a whole number, found `{trimmed}`")),
            Self::Float => {
                let parsed: f64 = trimmed
                    .parse()
                    .map_err(|_| format!("expected a number, found `{trimmed}`"))?;
                serde_json::Number::from_f64(parsed)
                    .map(Value::Number)
                    .ok_or_else(|| format!("`{trimmed}` is not a finite number"))
            }
            Self::List => {
                if trimmed.is_empty() {
                    // CFG-027: Java's `PropertyHelper.validateType` rejects an empty list.
                    return Err("a list property may not be set to an empty value".to_owned());
                }
                Ok(Value::Array(
                    trimmed
                        .split(',')
                        .map(|item| Value::String(item.trim().to_owned()))
                        .collect(),
                ))
            }
            Self::Enum(variants) => variants
                .iter()
                .find(|variant| variant.eq_ignore_ascii_case(trimmed))
                .map(|variant| Value::String((*variant).to_owned()))
                .ok_or_else(|| {
                    format!("expected one of {}, found `{trimmed}`", variants.join(", "))
                }),
            Self::String
            | Self::Path
            | Self::Secret
            | Self::Duration
            | Self::Socket
            | Self::Uuid
            | Self::Regex
            | Self::TimeZone
            | Self::Url => Ok(Value::String(raw.to_owned())),
        }
    }
}

impl fmt::Display for PropertyKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Enum(variants) => write!(f, "enum({})", variants.join("|")),
            other => f.write_str(other.as_str()),
        }
    }
}

/// A type that can appear as a leaf configuration property.
///
/// Implementing this for a type is what makes it usable in a
/// [`cdm_properties!`](crate::cdm_properties) `fields` block: it supplies the [`PropertyKind`]
/// for the registry and knows how to render its own default.
pub trait PropertyValue: Serialize {
    /// The kind of value this type accepts.
    fn kind() -> PropertyKind;

    /// Whether the property may legitimately be absent.
    fn optional() -> bool {
        false
    }

    /// The documented default, or `None` when the property has no default.
    ///
    /// The blanket behaviour serialises `self`; types whose `Serialize` redacts (notably
    /// [`Secret`](crate::Secret)) override this so that generated documentation shows the real
    /// built-in default. Only *defaults* ever reach this method — never a value a user supplied
    /// — so `SEC-001` is not at risk.
    fn display_value(&self) -> Option<String> {
        match serde_json::to_value(self).ok()? {
            Value::Null => None,
            Value::String(s) => Some(s),
            Value::Array(items) => {
                let rendered: Vec<String> = items
                    .iter()
                    .map(|item| match item {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    })
                    .collect();
                if rendered.is_empty() {
                    None
                } else {
                    Some(rendered.join(","))
                }
            }
            other => Some(other.to_string()),
        }
    }
}

macro_rules! impl_property_value {
    ($($ty:ty => $kind:expr),* $(,)?) => {
        $(impl PropertyValue for $ty {
            fn kind() -> PropertyKind {
                $kind
            }
        })*
    };
}

impl_property_value! {
    bool => PropertyKind::Bool,
    u8 => PropertyKind::Integer,
    u16 => PropertyKind::Integer,
    u32 => PropertyKind::Integer,
    u64 => PropertyKind::Integer,
    i32 => PropertyKind::Integer,
    i64 => PropertyKind::Integer,
    f64 => PropertyKind::Float,
    String => PropertyKind::String,
    std::path::PathBuf => PropertyKind::Path,
    std::net::SocketAddr => PropertyKind::Socket,
    Vec<String> => PropertyKind::List,
}

impl<T: PropertyValue> PropertyValue for Option<T> {
    fn kind() -> PropertyKind {
        T::kind()
    }

    fn optional() -> bool {
        true
    }

    fn display_value(&self) -> Option<String> {
        self.as_ref().and_then(PropertyValue::display_value)
    }
}

/// Where in the struct tree a set of properties sits.
///
/// The context carries the canonical prefix (`connect.origin`) and, for structures that are
/// instantiated more than once, the substitution for the `{side}` placeholder in legacy names
/// (`spark.cdm.connect.{side}.host`). One `SideConnect` definition therefore yields both the
/// origin and the target rows of the registry, with no duplicated declarations.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MetaContext {
    prefix: String,
    side: Option<&'static str>,
}

impl MetaContext {
    /// The context of the configuration root.
    pub fn root() -> Self {
        Self::default()
    }

    /// The context of a nested section, optionally rebinding `{side}`.
    #[must_use]
    pub fn child(&self, key: &str, side: Option<&'static str>) -> Self {
        let prefix = if self.prefix.is_empty() {
            key.to_owned()
        } else {
            format!("{}.{key}", self.prefix)
        };
        Self {
            prefix,
            side: side.or(self.side),
        }
    }

    /// The canonical prefix, e.g. `connect.origin.tls`.
    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    /// The current `{side}` substitution, if any.
    pub fn side(&self) -> Option<&'static str> {
        self.side
    }

    /// The canonical name of a property in this section.
    pub fn canonical(&self, key: &str) -> String {
        if self.prefix.is_empty() {
            key.to_owned()
        } else {
            format!("{}.{key}", self.prefix)
        }
    }

    /// Substitutes `{side}` in a legacy alias template.
    pub fn expand(&self, template: &str) -> String {
        match self.side {
            Some(side) => template.replace("{side}", side),
            None => template.to_owned(),
        }
    }
}

/// The literal facts a [`cdm_properties!`](crate::cdm_properties) field declaration supplies.
///
/// This is an implementation detail of the macro, public only because macro expansion happens in
/// the caller's crate. Construct [`PropertyMeta`] through [`PropertyMeta::build`] instead.
#[derive(Debug, Clone)]
pub struct PropertySpec {
    /// The last segment of the canonical name.
    pub key: &'static str,
    /// Legacy `spark.cdm.*` alias templates, possibly containing `{side}`.
    pub legacy: &'static [&'static str],
    /// The value kind.
    pub kind: PropertyKind,
    /// Whether the property may be absent.
    pub optional: bool,
    /// The rendered built-in default, if there is one.
    pub default: Option<String>,
    /// The unit the value is expressed in, e.g. `rows/s`.
    pub unit: Option<&'static str>,
    /// Whether the value is a credential (`CFG-012`, `SEC-001`).
    pub secret: bool,
    /// A note describing a default that is computed at run time, e.g. `num_cpus`.
    pub default_note: Option<&'static str>,
    /// An illustrative value.
    pub example: Option<&'static str>,
    /// The stability marker.
    pub stability: Stability,
    /// The doc-comment lines of the field, verbatim.
    pub doc: &'static [&'static str],
}

/// One row of the property registry (`CFG-002`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PropertyMeta {
    /// The canonical cdm-rs name, e.g. `connect.origin.host`.
    pub canonical: String,
    /// Accepted legacy Java names, e.g. `spark.cdm.connect.origin.host`.
    pub legacy: Vec<String>,
    /// The value kind.
    pub kind: PropertyKind,
    /// Whether the property may be absent.
    pub optional: bool,
    /// The rendered built-in default, if there is one.
    pub default_value: Option<String>,
    /// A note describing a default computed at run time, e.g. `num_cpus`.
    pub default_note: Option<&'static str>,
    /// The unit the value is expressed in.
    pub unit: Option<&'static str>,
    /// Whether the value is a credential.
    pub secret: bool,
    /// The first line of the field's documentation.
    pub summary: String,
    /// The whole of the field's documentation, as Markdown.
    pub description: String,
    /// An illustrative value.
    pub example: Option<&'static str>,
    /// The stability marker.
    pub stability: Stability,
}

impl PropertyMeta {
    /// Builds a registry row by resolving a [`PropertySpec`] against its [`MetaContext`].
    pub fn build(ctx: &MetaContext, spec: PropertySpec) -> Self {
        let description = spec
            .doc
            .iter()
            .map(|line| line.trim())
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_owned();
        let summary = description
            .lines()
            .take_while(|line| !line.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        Self {
            canonical: ctx.canonical(spec.key),
            legacy: spec.legacy.iter().map(|t| ctx.expand(t)).collect(),
            kind: spec.kind,
            optional: spec.optional,
            default_value: spec.default,
            default_note: spec.default_note,
            unit: spec.unit,
            secret: spec.secret,
            summary,
            description,
            example: spec.example,
            stability: spec.stability,
        }
    }

    /// The default as it should be shown to a user: redacted when the property is a secret.
    pub fn displayed_default(&self) -> Option<String> {
        match (&self.default_value, self.secret) {
            (Some(_), true) => Some(crate::secret::REDACTED.to_owned()),
            (value, _) => value.clone(),
        }
    }
}

/// A struct whose fields are configuration properties.
///
/// Implemented by [`cdm_properties!`](crate::cdm_properties); never by hand.
pub trait Properties: Default {
    /// Every property this struct and its sections contribute, in declaration order.
    fn properties(ctx: &MetaContext) -> Vec<PropertyMeta>;
}

// Tests may panic freely: a failed assertion *is* the reporting mechanism, and the no-panic rule
// (ERR-004) exists to protect production paths, not test bodies.
#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
mod tests {
    use super::*;

    #[test]
    fn cfg_002_property_kinds_coerce_untyped_values() {
        assert_eq!(
            PropertyKind::Bool.coerce(" TRUE ").unwrap(),
            Value::Bool(true)
        );
        assert_eq!(PropertyKind::Bool.coerce("0").unwrap(), Value::Bool(false));
        assert!(PropertyKind::Bool.coerce("maybe").is_err());
        assert_eq!(
            PropertyKind::Integer.coerce("-7").unwrap(),
            serde_json::json!(-7)
        );
        assert!(PropertyKind::Integer.coerce("7.5").is_err());
        assert_eq!(
            PropertyKind::Float.coerce("2.5").unwrap(),
            serde_json::json!(2.5)
        );
        assert!(PropertyKind::Float.coerce("abc").is_err());
        // `"nan".parse::<f64>()` succeeds, but JSON has no NaN, so it is still rejected.
        assert!(PropertyKind::Float.coerce("nan").is_err());
        assert_eq!(
            PropertyKind::Enum(&["JKS", "PEM"]).coerce("pem").unwrap(),
            Value::String("PEM".to_owned())
        );
        assert!(PropertyKind::Enum(&["JKS"]).coerce("PKCS12").is_err());
        assert_eq!(
            PropertyKind::String.coerce(" spaced ").unwrap(),
            Value::String(" spaced ".to_owned())
        );
    }

    #[test]
    fn cfg_027_a_list_property_may_not_be_empty() {
        assert!(PropertyKind::List.coerce("   ").is_err());
        assert_eq!(
            PropertyKind::List.coerce("a,b , c").unwrap(),
            serde_json::json!(["a", "b", "c"])
        );
    }

    #[test]
    fn cfg_002_context_expands_the_side_placeholder_and_nests_prefixes() {
        let root = MetaContext::root();
        assert_eq!(root.canonical("server"), "server");
        let connect = root.child("connect", None);
        let origin = connect.child("origin", Some("origin"));
        let tls = origin.child("tls", None);
        assert_eq!(tls.prefix(), "connect.origin.tls");
        assert_eq!(tls.side(), Some("origin"));
        assert_eq!(
            tls.expand("spark.cdm.connect.{side}.tls.enabled"),
            "spark.cdm.connect.origin.tls.enabled"
        );
        assert_eq!(tls.canonical("enabled"), "connect.origin.tls.enabled");
    }

    #[test]
    fn cfg_002_metadata_carries_summary_description_and_stability() {
        let meta = PropertyMeta::build(
            &MetaContext::root().child("perfops", None),
            PropertySpec {
                key: "num_parts",
                legacy: &["spark.cdm.perfops.numParts"],
                kind: PropertyKind::Integer,
                optional: false,
                default: Some("5000".to_owned()),
                unit: Some("parts"),
                secret: false,
                default_note: None,
                example: Some("10000"),
                stability: Stability::Stable,
                doc: &["Number of splits.", "", "Rule of thumb: size / 10 MB."],
            },
        );
        assert_eq!(meta.canonical, "perfops.num_parts");
        assert_eq!(meta.legacy, ["spark.cdm.perfops.numParts"]);
        assert_eq!(meta.summary, "Number of splits.");
        assert!(meta.description.contains("Rule of thumb"));
        assert_eq!(meta.stability, Stability::Stable);
        assert_eq!(meta.stability.to_string(), "stable");
        assert_eq!(meta.displayed_default().as_deref(), Some("5000"));
    }

    #[test]
    fn cfg_012_a_secret_default_is_redacted_when_displayed() {
        let meta = PropertyMeta::build(
            &MetaContext::root(),
            PropertySpec {
                key: "password",
                legacy: &[],
                kind: PropertyKind::Secret,
                optional: false,
                default: Some("cassandra".to_owned()),
                unit: None,
                secret: true,
                default_note: None,
                example: None,
                stability: Stability::Stable,
                doc: &["A credential."],
            },
        );
        assert_eq!(meta.displayed_default().as_deref(), Some("***"));
        assert_eq!(meta.default_value.as_deref(), Some("cassandra"));
    }

    #[test]
    fn cfg_002_property_value_renders_defaults_from_the_typed_value() {
        assert_eq!(
            PropertyValue::display_value(&5000_u64).as_deref(),
            Some("5000")
        );
        assert_eq!(PropertyValue::display_value(&true).as_deref(), Some("true"));
        assert_eq!(
            PropertyValue::display_value(&vec!["a".to_owned(), "b".to_owned()]).as_deref(),
            Some("a,b")
        );
        assert_eq!(PropertyValue::display_value(&Vec::<String>::new()), None);
        assert_eq!(PropertyValue::display_value(&Option::<String>::None), None);
        assert!(<Option<String> as PropertyValue>::optional());
        assert!(!<String as PropertyValue>::optional());
        assert_eq!(
            <Option<u32> as PropertyValue>::kind(),
            PropertyKind::Integer
        );
        assert_eq!(PropertyKind::Enum(&["a", "b"]).to_string(), "enum(a|b)");
    }
}
