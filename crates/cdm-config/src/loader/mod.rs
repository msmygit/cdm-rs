//! Layered configuration loading (`CFG-010`..`CFG-013`).
//!
//! # Precedence
//!
//! Layers are merged in the increasing order of precedence `CFG-010` fixes, regardless of the
//! order they were added to the loader:
//!
//! 1. built-in defaults (the `impl Default` the property macro generates),
//! 2. a config file — `.toml`, `.yaml`/`.yml`, `.json` or Java-style `.properties`,
//! 3. environment variables prefixed `CDM__`, with `__` as the nesting separator,
//! 4. `--set key=value` / `--conf key=value` overrides,
//! 5. explicit typed CLI flags,
//! 6. values supplied in an API request body.
//!
//! # How the merge works
//!
//! Every layer is *flattened* to canonical dotted keys before merging, so precedence is decided
//! per property rather than per section — setting `CDM__PERFOPS__BATCH_SIZE` does not discard the
//! `perfops.num_parts` from the file. The merged map is then unflattened into a JSON tree and
//! deserialised **once** into [`CdmConfig`]. The configuration is never serialised back out,
//! which is what makes a redacting [`Secret`](crate::Secret) safe (`SEC-001`).
//!
//! Unknown keys are reported, not ignored: an unknown `spark.cdm.*` key produces a warning naming
//! the closest known key, an unknown `spark.*` key is ignored silently because it is Spark tuning
//! that no longer applies, and anything else is a warning (`CFG-011`).

pub mod properties;

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use cdm_core::{Diagnostic, Severity};
use serde::Serialize;
use serde_json::{Map, Value};

use crate::meta::PropertyKind;
use crate::model::CdmConfig;
use crate::registry::PropertyRegistry;
use crate::secret::{self, SecretSource, SystemSecrets};

/// The diagnostic code every configuration finding carries (`ERR-002`).
pub const CODE: &str = "CDM-CONFIG";

/// Where a value came from (`CFG-010`, and the answer `cdm config explain` prints).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// The built-in default.
    Defaults,
    /// A configuration file.
    File(PathBuf),
    /// A `CDM__*` environment variable.
    Environment,
    /// A `--set` or `--conf` override.
    Override,
    /// An explicit typed CLI flag such as `--origin-host`.
    Flag,
    /// The body of an API request.
    ApiBody,
}

impl Source {
    /// The precedence rank, lowest first, exactly as `CFG-010` orders them.
    const fn rank(&self) -> u8 {
        match self {
            Self::Defaults => 0,
            Self::File(_) => 1,
            Self::Environment => 2,
            Self::Override => 3,
            Self::Flag => 4,
            Self::ApiBody => 5,
        }
    }
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Defaults => f.write_str("built-in default"),
            Self::File(path) => write!(f, "file {}", path.display()),
            Self::Environment => f.write_str("environment"),
            Self::Override => f.write_str("--set/--conf"),
            Self::Flag => f.write_str("command-line flag"),
            Self::ApiBody => f.write_str("API request body"),
        }
    }
}

/// One flattened assignment within a layer.
#[derive(Debug, Clone)]
struct Assignment {
    key: String,
    value: Value,
    location: Option<String>,
}

/// One source of configuration, already flattened to canonical keys.
#[derive(Debug, Clone)]
struct Layer {
    source: Source,
    assignments: Vec<Assignment>,
}

/// What a load produced.
#[derive(Debug)]
pub struct LoadOutcome {
    /// The configuration, absent only when a diagnostic made it unconstructable.
    pub config: Option<CdmConfig>,
    /// Every finding, in the order they were produced. Never fails fast (`CFG-021`).
    pub diagnostics: Vec<Diagnostic>,
    /// Which layer supplied each explicitly set property (`CFG-028`).
    pub sources: BTreeMap<String, Source>,
    /// Where within that layer the value was written — a file and line, an environment variable
    /// name, or the override as it was typed (`CFG-028`).
    pub locations: BTreeMap<String, String>,
}

impl LoadOutcome {
    /// Whether any diagnostic blocks the run.
    pub fn has_errors(&self) -> bool {
        self.diagnostics.iter().any(Diagnostic::is_blocking)
    }
}

/// Builds a [`CdmConfig`] from layered sources (`CFG-010`).
///
/// ```
/// use cdm_config::ConfigLoader;
///
/// let outcome = ConfigLoader::new()
///     .with_properties_str("spark.cdm.perfops.numParts 7\n", "cdm.properties")
///     .with_env([("CDM__PERFOPS__BATCH_SIZE".to_owned(), "9".to_owned())])
///     .load();
/// let config = outcome.config.unwrap();
/// assert_eq!(config.perfops.num_parts, 7);
/// assert_eq!(config.perfops.batch_size, 9);
/// ```
pub struct ConfigLoader {
    layers: Vec<Layer>,
    diagnostics: Vec<Diagnostic>,
    profile: Option<String>,
    strict: bool,
    compat_java: bool,
    secrets: Box<dyn SecretSource>,
}

impl fmt::Debug for ConfigLoader {
    /// Written by hand rather than derived, for two reasons: a `Box<dyn SecretSource>` is not
    /// `Debug`-derivable, and the layers hold values that have not yet been classified as
    /// secret, so printing them would be a `SEC-001` leak waiting to happen. Only the *shape* of
    /// the loader is shown.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConfigLoader")
            .field("layers", &self.layers.len())
            .field("diagnostics", &self.diagnostics.len())
            .field("profile", &self.profile)
            .field("strict", &self.strict)
            .field("compat_java", &self.compat_java)
            .field("secrets", &self.secrets)
            .finish()
    }
}

impl Default for ConfigLoader {
    fn default() -> Self {
        Self::new()
    }
}

impl ConfigLoader {
    /// A loader with nothing but the built-in defaults.
    pub fn new() -> Self {
        Self {
            layers: Vec::new(),
            diagnostics: Vec::new(),
            profile: None,
            strict: false,
            compat_java: false,
            secrets: Box::new(SystemSecrets),
        }
    }

    /// Selects a profile to deep-merge over the base of every structured file (`CFG-013`).
    #[must_use]
    pub fn with_profile(mut self, name: impl Into<String>) -> Self {
        self.profile = Some(name.into());
        self
    }

    /// Makes an unknown `spark.cdm.*` key an error rather than a warning (`--strict-config`).
    #[must_use]
    pub fn strict(mut self, strict: bool) -> Self {
        self.strict = strict;
        self
    }

    /// Restores Java CDM's silent coercion of an unrecognised consistency level (`CFG-161`).
    #[must_use]
    pub fn compat_java(mut self, compat: bool) -> Self {
        self.compat_java = compat;
        self
    }

    /// Replaces the resolver used for `env:`/`file:`/`exec:` secrets (`CFG-012`).
    #[must_use]
    pub fn with_secret_source(mut self, source: Box<dyn SecretSource>) -> Self {
        self.secrets = source;
        self
    }

    /// Reads a configuration file, choosing the parser from its extension (`CFG-010` layer 2).
    ///
    /// A `.properties` extension, or no extension at all, selects the Java parser.
    #[must_use]
    pub fn with_file(mut self, path: impl AsRef<Path>) -> Self {
        let path = path.as_ref();
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) => {
                self.diagnostics.push(
                    Diagnostic::error(CODE, "cannot read the configuration file")
                        .with_location(path.display().to_string())
                        .with_detail(error.to_string())
                        .with_rule("CFG-010"),
                );
                return self;
            }
        };
        let extension = path
            .extension()
            .and_then(std::ffi::OsStr::to_str)
            .unwrap_or("properties")
            .to_ascii_lowercase();
        let label = path.display().to_string();
        let tree = match extension.as_str() {
            "toml" => toml::from_str::<Value>(&text).map_err(|e| e.to_string()),
            "yaml" | "yml" => serde_yaml::from_str::<Value>(&text).map_err(|e| e.to_string()),
            "json" => serde_json::from_str::<Value>(&text).map_err(|e| e.to_string()),
            _ => {
                return self.with_properties_str_at(&text, Source::File(path.to_path_buf()), &label)
            }
        };
        match tree {
            Ok(tree) => self.with_tree(tree, Source::File(path.to_path_buf()), &label),
            Err(error) => {
                self.diagnostics.push(
                    Diagnostic::error(CODE, "the configuration file is not well-formed")
                        .with_location(label)
                        .with_detail(error)
                        .with_rule("CFG-010"),
                );
                self
            }
        }
    }

    /// Adds a Java `.properties` document that is already in memory (`CFG-011`).
    #[must_use]
    pub fn with_properties_str(self, text: &str, label: &str) -> Self {
        let source = Source::File(PathBuf::from(label));
        self.with_properties_str_at(text, source, label)
    }

    fn with_properties_str_at(mut self, text: &str, source: Source, label: &str) -> Self {
        let registry = PropertyRegistry::global();
        let mut assignments = Vec::new();
        for entry in properties::parse(text) {
            let location = format!("{label}:{}", entry.line);
            let Some(meta) = registry.resolve(&entry.key) else {
                self.diagnostics
                    .extend(unknown_key(&entry.key, &location, self.strict));
                continue;
            };
            // A value that cannot be coerced has already produced a diagnostic; dropping it lets
            // every later tier still run against the remaining keys (CFG-021).
            if let Some(value) = self.coerce(meta.kind, &entry.value, &meta.canonical, &location) {
                assignments.push(Assignment {
                    key: meta.canonical.clone(),
                    value,
                    location: Some(location),
                });
            }
        }
        self.layers.push(Layer {
            source,
            assignments,
        });
        self
    }

    /// Adds the `CDM__*` environment variables (`CFG-010` layer 3).
    ///
    /// Takes the variables rather than reading the process environment so that the precedence
    /// rules can be tested without mutating global state.
    #[must_use]
    pub fn with_env(self, vars: impl IntoIterator<Item = (String, String)>) -> Self {
        let pairs: Vec<(String, String, String)> = vars
            .into_iter()
            .filter_map(|(name, value)| {
                let key = name.strip_prefix("CDM__")?;
                Some((key.replace("__", ".").to_ascii_lowercase(), value, name))
            })
            .collect();
        self.with_string_pairs(pairs, Source::Environment)
    }

    /// Adds the process's own `CDM__*` environment variables.
    #[must_use]
    pub fn with_process_env(self) -> Self {
        let vars: Vec<(String, String)> = std::env::vars().collect();
        self.with_env(vars)
    }

    /// Adds `--set key=value` / `--conf key=value` overrides (`CFG-010` layer 4, `CLI-002`).
    #[must_use]
    pub fn with_overrides<I, S>(self, overrides: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.with_assignment_strings(overrides, Source::Override, "--set")
    }

    /// Adds explicit typed CLI flags, already resolved to canonical keys (`CFG-010` layer 5).
    #[must_use]
    pub fn with_flags<I, S>(self, flags: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.with_assignment_strings(flags, Source::Flag, "flag")
    }

    /// Adds the values supplied in an API request body (`CFG-010` layer 6).
    #[must_use]
    pub fn with_api_body(self, body: Value) -> Self {
        self.with_tree(body, Source::ApiBody, "request body")
    }

    fn with_assignment_strings<I, S>(mut self, items: I, source: Source, label: &str) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut pairs = Vec::new();
        for item in items {
            let item = item.as_ref();
            match item.split_once('=') {
                Some((key, value)) => {
                    let key = key.trim().to_owned();
                    // SEC-001: the provenance string is what `cdm config explain` prints, so it
                    // records *where* the value came from and never the value itself —
                    // `--set connect.origin.password=hunter2` must not survive anywhere.
                    let location = format!("{label} {key}");
                    pairs.push((key, value.to_owned(), location));
                }
                None => self.diagnostics.push(
                    Diagnostic::error(CODE, "an override must be written `key=value`")
                        .with_location(label.to_owned())
                        // Only the leading token is echoed: without an `=` there is nothing to
                        // say which half is the key, and the rest may be a credential.
                        .with_value(
                            item.split([' ', '\t', ':'])
                                .next()
                                .unwrap_or(item)
                                .to_owned(),
                        )
                        .with_rule("CFG-010"),
                ),
            }
        }
        self.with_string_pairs(pairs, source)
    }

    /// Adds a layer of `(key, raw value, where it was written)` triples.
    fn with_string_pairs(mut self, pairs: Vec<(String, String, String)>, source: Source) -> Self {
        let registry = PropertyRegistry::global();
        let mut assignments = Vec::new();
        for (key, raw, location) in pairs {
            let Some(meta) = registry.resolve(&key) else {
                self.diagnostics
                    .extend(unknown_key(&key, &location, self.strict));
                continue;
            };
            if let Some(value) = self.coerce(meta.kind, &raw, &meta.canonical, &location) {
                assignments.push(Assignment {
                    key: meta.canonical.clone(),
                    value,
                    location: Some(location),
                });
            }
        }
        self.layers.push(Layer {
            source,
            assignments,
        });
        self
    }

    /// Adds a structured document, applying the selected profile over its base (`CFG-013`).
    fn with_tree(mut self, mut tree: Value, source: Source, label: &str) -> Self {
        let profiles = tree
            .as_object_mut()
            .and_then(|object| object.remove("profiles"));

        let mut flat = Vec::new();
        flatten(&tree, String::new(), &mut flat);

        if let Some(name) = self.profile.clone() {
            match profiles.as_ref().and_then(|p| p.get(&name)) {
                Some(overlay) => flatten(overlay, String::new(), &mut flat),
                None if profiles.is_some() => self.diagnostics.push(
                    Diagnostic::error(CODE, "the requested profile is not defined")
                        .with_location(label.to_owned())
                        .with_value(name)
                        .with_rule("CFG-013")
                        .with_suggestion("check the `[profiles.<name>]` blocks in this file"),
                ),
                None => {}
            }
        }

        let registry = PropertyRegistry::global();
        let mut assignments = Vec::new();
        for (key, value) in flat {
            let Some(meta) = registry.resolve(&key) else {
                self.diagnostics
                    .extend(unknown_key(&key, label, self.strict));
                continue;
            };
            // A structured document may still spell a typed value as a string, and an enum
            // written in the wrong case must be normalised before deserialisation.
            let normalised = match (&value, meta.kind) {
                (Value::String(raw), _) => self.coerce(meta.kind, raw, &meta.canonical, label),
                (Value::Number(n), PropertyKind::BigInteger) => Some(Value::String(n.to_string())),
                _ => Some(value),
            };
            if let Some(value) = normalised {
                assignments.push(Assignment {
                    key: meta.canonical.clone(),
                    value,
                    location: Some(label.to_owned()),
                });
            }
        }
        self.layers.push(Layer {
            source,
            assignments,
        });
        self
    }

    /// Coerces a raw string, recording a diagnostic when it does not fit the property's kind.
    fn coerce(
        &mut self,
        kind: PropertyKind,
        raw: &str,
        canonical: &str,
        location: &str,
    ) -> Option<Value> {
        match kind.coerce(raw) {
            Ok(value) => Some(value),
            Err(reason) => {
                // CFG-161: Java silently coerced an unrecognised consistency level to
                // LOCAL_QUORUM. cdm-rs rejects it, unless `--compat-java` asks for the old
                // behaviour.
                if self.compat_java && canonical.starts_with("perfops.consistency.") {
                    self.diagnostics.push(
                        Diagnostic::warning(CODE, "unrecognised consistency level coerced")
                            .with_location(location.to_owned())
                            .with_value(raw.to_owned())
                            .with_detail(reason)
                            .with_rule("CFG-161")
                            .with_suggestion(
                                "`--compat-java` is in effect, so `LOCAL_QUORUM` was used; \
                                 drop the flag to make this an error",
                            ),
                    );
                    return Some(Value::String("LOCAL_QUORUM".to_owned()));
                }
                self.diagnostics.push(
                    Diagnostic::error(CODE, format!("`{canonical}` has an unusable value"))
                        .with_location(location.to_owned())
                        .with_value(raw.to_owned())
                        .with_detail(reason)
                        .with_rule(if matches!(kind, PropertyKind::List) {
                            "CFG-027"
                        } else if canonical.starts_with("perfops.consistency.") {
                            "CFG-161"
                        } else {
                            "CFG-020"
                        }),
                );
                None
            }
        }
    }

    /// Merges every layer, resolves secrets and deserialises (`CFG-010`, `CFG-012`).
    #[must_use]
    pub fn load(mut self) -> LoadOutcome {
        self.layers.sort_by_key(|layer| layer.source.rank());

        let mut merged: BTreeMap<String, Value> = BTreeMap::new();
        let mut sources: BTreeMap<String, Source> = BTreeMap::new();
        let mut locations: BTreeMap<String, String> = BTreeMap::new();
        for layer in &self.layers {
            for assignment in &layer.assignments {
                merged.insert(assignment.key.clone(), assignment.value.clone());
                sources.insert(assignment.key.clone(), layer.source.clone());
                if let Some(location) = &assignment.location {
                    locations.insert(assignment.key.clone(), location.clone());
                }
            }
        }

        self.resolve_secrets(&mut merged, &sources);

        let tree = unflatten(&merged);
        let config = match serde_json::from_value::<CdmConfig>(tree) {
            Ok(config) => Some(config),
            Err(error) => {
                self.diagnostics.push(
                    Diagnostic::error(CODE, "the merged configuration is not valid")
                        .with_detail(error.to_string())
                        .with_rule("CFG-020"),
                );
                None
            }
        };

        LoadOutcome {
            config,
            diagnostics: self.diagnostics,
            sources,
            locations,
        }
    }

    /// Resolves `env:`/`file:`/`exec:` indirection for every property marked secret (`CFG-012`).
    fn resolve_secrets(
        &mut self,
        merged: &mut BTreeMap<String, Value>,
        sources: &BTreeMap<String, Source>,
    ) {
        for meta in PropertyRegistry::global().secrets() {
            let Some(Value::String(raw)) = merged.get(&meta.canonical) else {
                continue;
            };
            match secret::resolve(raw, self.secrets.as_ref()) {
                Ok(resolved) => {
                    merged.insert(meta.canonical.clone(), Value::String(resolved));
                }
                Err(reason) => {
                    // SEC-001: the diagnostic names the property and the failure, never the
                    // value — an unresolvable `env:` spec is safe to quote, a literal is not.
                    self.diagnostics.push(
                        Diagnostic::error(CODE, "cannot resolve a secret")
                            .with_location(
                                sources
                                    .get(&meta.canonical)
                                    .map_or_else(|| "configuration".to_owned(), Source::to_string),
                            )
                            .with_detail(format!("{}: {reason}", meta.canonical))
                            .with_rule("CFG-012")
                            .with_suggestion(
                                "use `env:VAR`, `file:/path` or `exec:command`, or the value \
                                 itself",
                            ),
                    );
                    merged.remove(&meta.canonical);
                }
            }
        }
    }
}

/// The diagnostics an unrecognised key produces (`CFG-011`).
fn unknown_key(key: &str, location: &str, strict: bool) -> Vec<Diagnostic> {
    // Spark tuning keys are ignored silently: they were never cdm-rs's to interpret, and a
    // migrated Java configuration is full of them.
    if key.starts_with("spark.") && !key.starts_with("spark.cdm.") {
        return Vec::new();
    }
    let severity = if strict {
        Severity::Error
    } else {
        Severity::Warning
    };
    let mut diagnostic = Diagnostic::new(CODE, severity, "unknown property")
        .with_location(location.to_owned())
        .with_value(key.to_owned())
        .with_rule("CFG-011");
    if let Some(suggestion) = PropertyRegistry::global().closest_match(key) {
        diagnostic = diagnostic.with_suggestion(format!("did you mean `{suggestion}`?"));
    }
    vec![diagnostic]
}

/// Flattens a JSON tree to dotted keys, recursing into objects only.
fn flatten(value: &Value, prefix: String, out: &mut Vec<(String, Value)>) {
    match value {
        Value::Object(object) => {
            for (key, child) in object {
                let path = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                flatten(child, path, out);
            }
        }
        leaf => {
            if !prefix.is_empty() {
                out.push((prefix, leaf.clone()));
            }
        }
    }
}

/// Rebuilds a nested JSON object from dotted keys.
fn unflatten(flat: &BTreeMap<String, Value>) -> Value {
    let mut root = Map::new();
    for (key, value) in flat {
        let segments: Vec<&str> = key.split('.').collect();
        insert_path(&mut root, &segments, value.clone());
    }
    Value::Object(root)
}

/// Inserts `value` at `segments` within `object`, creating intermediate objects.
fn insert_path(object: &mut Map<String, Value>, segments: &[&str], value: Value) {
    match segments {
        [] => {}
        [last] => {
            object.insert((*last).to_owned(), value);
        }
        [head, rest @ ..] => {
            let entry = object
                .entry((*head).to_owned())
                .or_insert_with(|| Value::Object(Map::new()));
            // A leaf and a section cannot share a name, because canonical names come from the
            // registry and the registry comes from a Rust struct tree; this keeps the function
            // total anyway.
            if !entry.is_object() {
                *entry = Value::Object(Map::new());
            }
            if let Some(child) = entry.as_object_mut() {
                insert_path(child, rest, value);
            }
        }
    }
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
    use std::io::Write as _;

    use super::*;

    #[test]
    fn cfg_010_every_layer_beats_the_one_below_it() {
        let outcome = ConfigLoader::new()
            .with_api_body(serde_json::json!({"perfops": {"num_parts": 6}}))
            .with_flags(["perfops.num_parts=5", "perfops.batch_size=50"])
            .with_overrides(["perfops.num_parts=4", "perfops.fetch_size=40"])
            .with_env([
                ("CDM__PERFOPS__NUM_PARTS".to_owned(), "3".to_owned()),
                ("CDM__PERFOPS__ERROR_LIMIT".to_owned(), "30".to_owned()),
            ])
            .with_properties_str(
                "spark.cdm.perfops.numParts 2\nspark.cdm.perfops.ratelimit.origin 20\n",
                "cdm.properties",
            )
            .load();

        assert!(!outcome.has_errors(), "{:?}", outcome.diagnostics);
        let config = outcome.config.unwrap();
        // The API body wins outright.
        assert_eq!(config.perfops.num_parts, 6);
        // Each lower layer still supplies what nothing above it set.
        assert_eq!(config.perfops.batch_size, 50);
        assert_eq!(config.perfops.fetch_size, 40);
        assert_eq!(config.perfops.error_limit, 30);
        assert_eq!(config.perfops.ratelimit.origin, 20);
        // And the built-in default survives where no layer spoke.
        assert_eq!(config.perfops.ratelimit.target, 20_000);

        assert_eq!(
            outcome.sources.get("perfops.num_parts"),
            Some(&Source::ApiBody)
        );
        assert_eq!(
            outcome.sources.get("perfops.error_limit"),
            Some(&Source::Environment)
        );
        // Nothing recorded a source for a property no layer mentioned, which is what makes
        // `cdm config explain` able to say "built-in default" (CFG-028).
        assert!(!outcome.sources.contains_key("perfops.ratelimit.target"));
    }

    #[test]
    fn cfg_010_the_order_layers_are_added_in_does_not_matter() {
        let low_first = ConfigLoader::new()
            .with_properties_str("spark.cdm.perfops.numParts 2\n", "f")
            .with_overrides(["perfops.num_parts=4"])
            .load();
        let high_first = ConfigLoader::new()
            .with_overrides(["perfops.num_parts=4"])
            .with_properties_str("spark.cdm.perfops.numParts 2\n", "f")
            .load();
        assert_eq!(low_first.config.unwrap().perfops.num_parts, 4);
        assert_eq!(high_first.config.unwrap().perfops.num_parts, 4);
    }

    #[test]
    fn cfg_010_environment_variables_nest_on_double_underscores() {
        let outcome = ConfigLoader::new()
            .with_env([
                (
                    "CDM__CONNECT__ORIGIN__HOST".to_owned(),
                    "10.0.0.1".to_owned(),
                ),
                (
                    "CDM__CONNECT__ORIGIN__TLS__TRUSTSTORE__TYPE".to_owned(),
                    "PEM".to_owned(),
                ),
                ("PATH".to_owned(), "/usr/bin".to_owned()),
            ])
            .load();
        let config = outcome.config.unwrap();
        assert_eq!(config.connect.origin.host, "10.0.0.1");
        assert_eq!(
            config.connect.origin.tls.truststore.store_type,
            crate::types::TrustStoreType::Pem
        );
        assert!(outcome.diagnostics.is_empty(), "{:?}", outcome.diagnostics);
    }

    #[test]
    fn cfg_010_toml_yaml_and_json_files_all_load() {
        let dir = tempfile::tempdir().unwrap();
        let write = |name: &str, body: &str| {
            let path = dir.path().join(name);
            let mut file = std::fs::File::create(&path).unwrap();
            file.write_all(body.as_bytes()).unwrap();
            path
        };

        let toml_path = write(
            "cdm.toml",
            "[perfops]\nnum_parts = 11\n[schema.origin]\nkeyspace_table = \"ks.a\"\n",
        );
        let yaml_path = write("cdm.yaml", "perfops:\n  num_parts: 12\n");
        let json_path = write("cdm.json", "{\"perfops\": {\"num_parts\": 13}}");

        for (path, expected) in [(toml_path, 11), (yaml_path, 12), (json_path, 13)] {
            let outcome = ConfigLoader::new().with_file(&path).load();
            assert!(!outcome.has_errors(), "{:?}", outcome.diagnostics);
            assert_eq!(outcome.config.unwrap().perfops.num_parts, expected);
        }
    }

    #[test]
    fn cfg_010_a_missing_or_malformed_file_is_a_diagnostic_not_a_panic() {
        let outcome = ConfigLoader::new().with_file("/no/such/cdm.toml").load();
        assert!(outcome.has_errors());
        assert!(outcome.diagnostics[0].title.contains("cannot read"));

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("broken.toml");
        std::fs::write(&path, "this is not toml =").unwrap();
        let outcome = ConfigLoader::new().with_file(&path).load();
        assert!(outcome.has_errors());
        assert!(outcome.diagnostics[0].title.contains("well-formed"));
    }

    #[test]
    fn cfg_011_the_whole_java_namespace_is_accepted() {
        let registry = PropertyRegistry::global();
        let document: String = registry
            .all()
            .iter()
            .filter_map(|meta| {
                let alias = meta.legacy.first()?;
                let value = sample_value(meta.kind);
                Some(format!("{alias} {value}\n"))
            })
            .collect();

        let outcome = ConfigLoader::new()
            .with_properties_str(&document, "everything.properties")
            .load();
        assert!(outcome.diagnostics.is_empty(), "{:#?}", outcome.diagnostics);
        assert!(outcome.config.is_some());
    }

    fn sample_value(kind: PropertyKind) -> String {
        match kind {
            PropertyKind::Bool => "true".to_owned(),
            PropertyKind::Integer | PropertyKind::BigInteger => "1".to_owned(),
            PropertyKind::Float => "1.5".to_owned(),
            PropertyKind::Duration => "5s".to_owned(),
            PropertyKind::Enum(variants) => (*variants.first().unwrap_or(&"")).to_owned(),
            PropertyKind::Socket => "127.0.0.1:1".to_owned(),
            PropertyKind::Uuid => "1a2b3c4d-5e6f-4a8b-9c0d-1e2f3a4b5c6d".to_owned(),
            PropertyKind::List => "a,b".to_owned(),
            _ => "x".to_owned(),
        }
    }

    #[test]
    fn cfg_011_an_unknown_cdm_key_warns_with_the_closest_match() {
        let outcome = ConfigLoader::new()
            .with_properties_str("spark.cdm.connect.orgin.host  10.0.0.1\n", "cdm.properties")
            .load();
        assert!(!outcome.has_errors());
        let diagnostic = &outcome.diagnostics[0];
        assert_eq!(diagnostic.severity, Severity::Warning);
        assert_eq!(diagnostic.rule.as_deref(), Some("CFG-011"));
        assert_eq!(diagnostic.location.as_deref(), Some("cdm.properties:1"));
        assert_eq!(
            diagnostic.suggestion.as_deref(),
            Some("did you mean `spark.cdm.connect.origin.host`?")
        );
    }

    #[test]
    fn cfg_011_strict_config_turns_the_warning_into_an_error() {
        let outcome = ConfigLoader::new()
            .strict(true)
            .with_properties_str("spark.cdm.connect.orgin.host  1\n", "cdm.properties")
            .load();
        assert!(outcome.has_errors());
    }

    #[test]
    fn cfg_011_non_cdm_spark_keys_are_ignored_silently() {
        let outcome = ConfigLoader::new()
            .strict(true)
            .with_properties_str(
                "spark.executor.memory 8g\nspark.master local[*]\nspark.cdm.perfops.numParts 3\n",
                "cdm.properties",
            )
            .load();
        assert!(outcome.diagnostics.is_empty(), "{:?}", outcome.diagnostics);
        assert_eq!(outcome.config.unwrap().perfops.num_parts, 3);
    }

    #[test]
    fn cfg_013_a_profile_is_deep_merged_over_the_base() {
        let document = "\
[perfops]
num_parts = 100
batch_size = 5

[profiles.prod.perfops]
num_parts = 50000

[profiles.prod.connect.target]
host = \"prod-target\"
";
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cdm.toml");
        std::fs::write(&path, document).unwrap();

        let base = ConfigLoader::new().with_file(&path).load();
        let config = base.config.unwrap();
        assert_eq!(config.perfops.num_parts, 100);
        assert_eq!(config.connect.target.host, "localhost");

        let prod = ConfigLoader::new()
            .with_profile("prod")
            .with_file(&path)
            .load();
        assert!(!prod.has_errors(), "{:?}", prod.diagnostics);
        let config = prod.config.unwrap();
        assert_eq!(config.perfops.num_parts, 50_000);
        // The base is merged, not replaced.
        assert_eq!(config.perfops.batch_size, 5);
        assert_eq!(config.connect.target.host, "prod-target");
    }

    #[test]
    fn cfg_013_an_undefined_profile_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cdm.toml");
        std::fs::write(
            &path,
            "[profiles.dev]\n[profiles.dev.perfops]\nnum_parts = 1\n",
        )
        .unwrap();
        let outcome = ConfigLoader::new()
            .with_profile("staging")
            .with_file(&path)
            .load();
        assert!(outcome.has_errors());
        assert_eq!(outcome.diagnostics[0].rule.as_deref(), Some("CFG-013"));
    }

    #[test]
    fn cfg_012_secret_indirection_is_resolved_during_the_load() {
        #[derive(Debug)]
        struct Fixed;
        impl SecretSource for Fixed {
            fn env(&self, name: &str) -> Result<String, String> {
                Ok(format!("env-{name}"))
            }
            fn file(&self, _path: &Path) -> Result<String, String> {
                Ok("from-file".to_owned())
            }
            fn exec(&self, _command: &str) -> Result<String, String> {
                Err("no shell here".to_owned())
            }
        }

        let outcome = ConfigLoader::new()
            .with_secret_source(Box::new(Fixed))
            .with_overrides([
                "connect.origin.password=env:ORIGIN_PW",
                "connect.target.password=file:/run/secrets/pw",
                "connect.origin.tls.keystore.password=exec:vault read",
            ])
            .load();

        let config = outcome.config.unwrap();
        assert_eq!(config.connect.origin.password.expose(), "env-ORIGIN_PW");
        assert_eq!(config.connect.target.password.expose(), "from-file");
        // The unresolvable one is reported and dropped, not silently kept as `exec:...`.
        assert!(config.connect.origin.tls.keystore.password.is_none());
        let failure = outcome
            .diagnostics
            .iter()
            .find(|d| d.rule.as_deref() == Some("CFG-012"))
            .unwrap();
        assert!(failure.is_blocking());
        assert!(!format!("{failure}").contains("vault read"));
    }

    #[test]
    fn cfg_012_a_literal_password_survives_the_load_unredacted() {
        let outcome = ConfigLoader::new()
            .with_overrides(["connect.origin.password=hunter2"])
            .load();
        let config = outcome.config.unwrap();
        assert_eq!(config.connect.origin.password.expose(), "hunter2");
        assert_eq!(config.connect.origin.password.to_string(), "***");
    }

    #[test]
    fn cfg_161_an_unrecognised_consistency_level_is_an_error_unless_compat_java() {
        let strict = ConfigLoader::new()
            .with_overrides(["perfops.consistency.read=LOCAL_QUOROM"])
            .load();
        assert!(strict.has_errors());
        assert_eq!(strict.diagnostics[0].rule.as_deref(), Some("CFG-161"));
        // The bad value is dropped, so the default still applies and later tiers can still run.
        assert_eq!(
            strict.config.unwrap().perfops.consistency.read,
            crate::types::ConsistencyLevel::LocalQuorum
        );

        let compat = ConfigLoader::new()
            .compat_java(true)
            .with_overrides(["perfops.consistency.read=LOCAL_QUOROM"])
            .load();
        assert!(!compat.has_errors());
        assert_eq!(compat.diagnostics[0].severity, Severity::Warning);
        assert_eq!(
            compat.config.unwrap().perfops.consistency.read,
            crate::types::ConsistencyLevel::LocalQuorum
        );
    }

    #[test]
    fn cfg_027_an_empty_list_value_is_rejected() {
        let outcome = ConfigLoader::new()
            .with_properties_str("spark.cdm.schema.origin.column.skip\n", "cdm.properties")
            .load();
        assert!(outcome.has_errors());
        assert_eq!(outcome.diagnostics[0].rule.as_deref(), Some("CFG-027"));
    }

    #[test]
    fn cfg_012_no_provenance_record_or_diagnostic_carries_a_credential() {
        // Assembled rather than written out, so that no `<key><separator><high-entropy value>`
        // literal exists in the source for a secret scanner to flag. The two stand-ins are
        // deliberately distinctive so the assertions below cannot pass by accident.
        let (origin_pw, keystore_pw) = ("s3kr1t-origin", "s3kr1t-keystore");
        let overrides = [
            format!("connect.origin.password={origin_pw}"),
            format!("connect.target.tls.keystore.password={keystore_pw}"),
            "perfops.numparts=7".to_owned(),
            // A malformed override may hold a credential too, and must not be echoed whole.
            format!("connect.origin.password:{origin_pw}"),
        ];
        let outcome = ConfigLoader::new().with_overrides(overrides).load();

        // Provenance says where, never what.
        assert_eq!(
            outcome
                .locations
                .get("connect.origin.password")
                .map(String::as_str),
            Some("--set connect.origin.password")
        );

        let rendered = format!(
            "{:?} {:?} {}",
            outcome.locations,
            outcome.sources,
            outcome
                .diagnostics
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n")
        );
        for credential in [origin_pw, keystore_pw] {
            assert!(
                !rendered.contains(credential),
                "{credential} leaked:\n{rendered}"
            );
        }
        // The misspelled key is still named, so the suggestion is still useful.
        assert!(rendered.contains("perfops.numparts"));
    }

    #[test]
    fn cfg_010_an_override_without_an_equals_sign_is_reported() {
        let outcome = ConfigLoader::new()
            .with_overrides(["perfops.num_parts"])
            .load();
        assert!(outcome.has_errors());
        assert!(outcome.diagnostics[0].title.contains("key=value"));
    }

    #[test]
    fn cfg_010_flattening_and_unflattening_round_trip() {
        let tree = serde_json::json!({"a": {"b": 1, "c": {"d": [1, 2]}}, "e": true});
        let mut flat = Vec::new();
        flatten(&tree, String::new(), &mut flat);
        let map: BTreeMap<String, Value> = flat.into_iter().collect();
        assert_eq!(map["a.b"], serde_json::json!(1));
        assert_eq!(map["a.c.d"], serde_json::json!([1, 2]));
        assert_eq!(unflatten(&map), tree);
    }
}
