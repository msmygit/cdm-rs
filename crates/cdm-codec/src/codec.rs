//! The codec registry (`CDC-030`, `CDC-031`) and the [`Converter`] a conversion plan holds.
//!
//! A codec is registered as a [`CodecPlugin`] in `cdm-core`'s [`Registry`], which is the one
//! public path both built-ins and third parties use (`PLG-010`). [`CodecRegistry`] is the
//! type-aware index built from it: it parses each plugin's `(from, to)` [`TypePair`]s into
//! [`CqlTypeInfo`] and resolves them, **once at startup**, into the [`Converter`] the conversion
//! plan holds for the lifetime of the run (`CDC-010`).

use std::fmt;
use std::sync::Arc;

use cdm_core::{CdmError, CodecPlugin, ErrorKind, RawCell, Registry, TypePair};
use serde::Serialize;

use crate::types::CqlTypeInfo;

/// Converts one cell from an origin type to a target type.
///
/// Resolved once per column pair at startup and then invoked per row, so an implementation must
/// not allocate more than the converted value requires (`ARCHITECTURE.md` §5.5).
pub trait Converter: fmt::Debug + Send + Sync + 'static {
    /// The codec this converter belongs to, e.g. `INT_STRING`. Used in diagnostics and in
    /// `cdm codecs list` (`CDC-031`).
    fn name(&self) -> &'static str;

    /// Converts one cell. A `NULL` input must produce a `NULL` output: Java's codecs all return
    /// `null` for `null`, and `MIG-012` turns that into an `UNSET` binding downstream.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::TypeConversion`] when the value cannot be represented in the target
    /// type. This is a record-level failure: the engine counts `ERROR` and continues.
    fn convert(&self, value: &RawCell) -> Result<RawCell, CdmError>;
}

/// A stateless converter, defined by a function over the serialised bytes of a non-null value.
pub(crate) struct FnConverter {
    pub(crate) codec: &'static str,
    pub(crate) convert: fn(&[u8]) -> Result<Vec<u8>, CdmError>,
}

impl fmt::Debug for FnConverter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FnConverter")
            .field("codec", &self.codec)
            .finish_non_exhaustive()
    }
}

impl Converter for FnConverter {
    fn name(&self) -> &'static str {
        self.codec
    }

    fn convert(&self, value: &RawCell) -> Result<RawCell, CdmError> {
        match value.bytes() {
            None => Ok(RawCell::NULL),
            Some(bytes) => Ok(RawCell::new((self.convert)(bytes)?)),
        }
    }
}

/// Adapts a [`CodecPlugin`] — which is keyed by string type names, because `cdm-core` may not
/// depend on this crate — to the [`Converter`] a plan holds.
struct PluginConverter {
    plugin: Arc<dyn CodecPlugin>,
    pair: TypePair,
}

impl fmt::Debug for PluginConverter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PluginConverter")
            .field("codec", &self.plugin.name())
            .field("pair", &self.pair)
            .finish()
    }
}

impl Converter for PluginConverter {
    fn name(&self) -> &'static str {
        self.plugin.name()
    }

    fn convert(&self, value: &RawCell) -> Result<RawCell, CdmError> {
        self.plugin.convert(&self.pair, value)
    }
}

/// One `(codec, origin type, target type)` registration.
pub struct CodecEntry {
    codec: &'static str,
    provider: &'static str,
    origin: CqlTypeInfo,
    target: CqlTypeInfo,
    converter: Arc<dyn Converter>,
}

impl fmt::Debug for CodecEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CodecEntry")
            .field("codec", &self.codec)
            .field("provider", &self.provider)
            .field("origin", &self.origin)
            .field("target", &self.target)
            .finish_non_exhaustive()
    }
}

impl CodecEntry {
    /// The codec's registration name, e.g. `INT_STRING`.
    pub const fn codec(&self) -> &'static str {
        self.codec
    }

    /// Who supplies the codec — `cdm-codec` for built-ins.
    pub const fn provider(&self) -> &'static str {
        self.provider
    }

    /// The origin type this entry converts from.
    pub const fn origin(&self) -> &CqlTypeInfo {
        &self.origin
    }

    /// The target type this entry converts to.
    pub const fn target(&self) -> &CqlTypeInfo {
        &self.target
    }

    /// The converter itself.
    pub const fn converter(&self) -> &Arc<dyn Converter> {
        &self.converter
    }
}

/// The serialisable view of a registration, as `cdm codecs list` prints it and
/// `GET /v1/codecs` returns it (`CDC-031`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CodecDescription {
    /// The codec's registration name.
    pub codec: String,
    /// Who supplies it.
    pub provider: String,
    /// The origin type, as CQL spells it.
    pub from: String,
    /// The target type, as CQL spells it.
    pub to: String,
}

/// Every conversion available to a run, indexed by type pair (`CDC-030`).
///
/// ```
/// use cdm_codec::{Codecset, CodecRegistry, CqlTypeInfo};
///
/// let registry = CodecRegistry::with_builtins(&[Codecset::IntString], None)?;
/// assert!(registry
///     .converter(&CqlTypeInfo::Int, &CqlTypeInfo::Text)
///     .is_some());
/// // BIGINT_BIGINTEGER is always registered, whether or not it was asked for.
/// assert!(registry
///     .converter(&CqlTypeInfo::BigInt, &CqlTypeInfo::VarInt)
///     .is_some());
/// # Ok::<(), cdm_core::CdmError>(())
/// ```
#[derive(Debug, Default, Clone)]
pub struct CodecRegistry {
    entries: Arc<Vec<CodecEntry>>,
}

impl CodecRegistry {
    /// Builds the type-aware index from a `cdm-core` [`Registry`] (`CDC-030`).
    ///
    /// Every registered [`CodecPlugin`] contributes its [`TypePair`]s, whose string type names are
    /// parsed into [`CqlTypeInfo`] here. A pair claimed by two codecs is a startup error, naming
    /// both (`PLG-010`).
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Config`] when a plugin declares a type name that does not parse, or
    /// when two plugins claim the same pair.
    pub fn from_registry(registry: &Registry) -> Result<Self, CdmError> {
        let mut entries: Vec<CodecEntry> = Vec::new();
        for plugin in registry.codecs() {
            for pair in plugin.conversions() {
                let origin = parse_pair_side(&pair.origin, plugin.name())?;
                let target = parse_pair_side(&pair.target, plugin.name())?;
                if let Some(existing) = entries
                    .iter()
                    .find(|e| e.origin.same_type(&origin) && e.target.same_type(&target))
                {
                    return Err(CdmError::new(
                        ErrorKind::Config,
                        format!(
                            "conversion {origin} -> {target} is claimed by both codec `{}` (from \
                             `{}`) and codec `{}` (from `{}`)",
                            existing.codec,
                            existing.provider,
                            plugin.name(),
                            plugin.provider()
                        ),
                    )
                    .with_context(|c| c.with_config_key("transform.codecs")));
                }
                entries.push(CodecEntry {
                    codec: plugin.name(),
                    provider: plugin.provider(),
                    origin,
                    target,
                    converter: Arc::new(PluginConverter {
                        plugin: Arc::clone(plugin),
                        pair,
                    }),
                });
            }
        }
        Ok(Self {
            entries: Arc::new(entries),
        })
    }

    /// Builds a registry over the named built-in codecs, through the same public registration path
    /// a third-party crate uses (`CDC-030`).
    ///
    /// `BIGINT_BIGINTEGER` is added whether or not it is named, because reading collection
    /// writetimes needs it (`CDC-020`).
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Config`] when `TIMESTAMP_STRING_FORMAT` is requested without a
    /// [`TimestampFormat`](crate::TimestampFormat) (`CDC-021`), or when registration conflicts.
    pub fn with_builtins(
        enabled: &[crate::builtin::Codecset],
        timestamp_format: Option<crate::builtin::TimestampFormat>,
    ) -> Result<Self, CdmError> {
        Self::from_registry(&crate::builtin::registry_with_builtins(
            enabled,
            timestamp_format,
        )?)
    }

    /// The converter for one type pair, if a codec claims it.
    ///
    /// Frozen-ness is not part of the match, for the reason [`CqlTypeInfo::same_type`] documents.
    pub fn converter(
        &self,
        origin: &CqlTypeInfo,
        target: &CqlTypeInfo,
    ) -> Option<&Arc<dyn Converter>> {
        self.entries
            .iter()
            .find(|e| e.origin.same_type(origin) && e.target.same_type(target))
            .map(CodecEntry::converter)
    }

    /// Every registration, in registration order.
    pub fn entries(&self) -> &[CodecEntry] {
        &self.entries
    }

    /// Every registration as a serialisable description (`CDC-031`).
    pub fn descriptions(&self) -> Vec<CodecDescription> {
        self.entries
            .iter()
            .map(|e| CodecDescription {
                codec: e.codec.to_owned(),
                provider: e.provider.to_owned(),
                from: e.origin.to_string(),
                to: e.target.to_string(),
            })
            .collect()
    }

    /// Whether nothing is registered.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The number of registered conversions.
    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

fn parse_pair_side(text: &str, codec: &str) -> Result<CqlTypeInfo, CdmError> {
    CqlTypeInfo::parse(text).map_err(|e| {
        CdmError::new(
            ErrorKind::Config,
            format!("codec `{codec}` declares the unparseable CQL type `{text}`: {e}"),
        )
        .with_context(|c| c.with_config_key("transform.codecs"))
    })
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
    use cdm_core::Plugin;

    /// A third-party codec, registered through exactly the API a built-in uses.
    #[derive(Debug)]
    struct ThirdParty {
        pair: TypePair,
    }

    impl Plugin for ThirdParty {
        fn name(&self) -> &'static str {
            "ROT13"
        }

        fn provider(&self) -> &'static str {
            "example-plugin"
        }
    }

    impl CodecPlugin for ThirdParty {
        fn conversions(&self) -> Vec<TypePair> {
            vec![self.pair.clone()]
        }

        fn convert(&self, _pair: &TypePair, value: &RawCell) -> Result<RawCell, CdmError> {
            let Some(bytes) = value.bytes() else {
                return Ok(RawCell::NULL);
            };
            let rotated: Vec<u8> = bytes
                .iter()
                .map(|b| match b {
                    b'a'..=b'z' => (b - b'a' + 13) % 26 + b'a',
                    other => *other,
                })
                .collect();
            Ok(RawCell::new(rotated))
        }
    }

    fn third_party(origin: &str, target: &str) -> Arc<dyn CodecPlugin> {
        Arc::new(ThirdParty {
            pair: TypePair::new(origin, target),
        })
    }

    #[test]
    fn cdc_030_a_third_party_codec_needs_no_change_to_cdm_rs() {
        let core = Registry::builder()
            .register_codec(third_party("text", "text"))
            .build()
            .unwrap();
        let registry = CodecRegistry::from_registry(&core).unwrap();
        let converter = registry
            .converter(&CqlTypeInfo::Text, &CqlTypeInfo::Text)
            .unwrap();
        assert_eq!(converter.name(), "ROT13");
        assert_eq!(
            converter.convert(&RawCell::new(b"abc".to_vec())).unwrap(),
            RawCell::new(b"nop".to_vec())
        );
        assert_eq!(converter.convert(&RawCell::NULL).unwrap(), RawCell::NULL);
    }

    #[test]
    fn cdc_030_a_type_pair_claimed_twice_is_a_startup_error_naming_both_codecs() {
        let core = Registry::builder()
            .register_codec(third_party("text", "text"))
            .register_codec(Arc::new(Twin))
            .build()
            .unwrap();
        let error = CodecRegistry::from_registry(&core).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Config);
        assert!(error.to_string().contains("ROT13"), "{error}");
        assert!(error.to_string().contains("TWIN"), "{error}");
    }

    #[derive(Debug)]
    struct Twin;

    impl Plugin for Twin {
        fn name(&self) -> &'static str {
            "TWIN"
        }

        fn provider(&self) -> &'static str {
            "other-plugin"
        }
    }

    impl CodecPlugin for Twin {
        fn conversions(&self) -> Vec<TypePair> {
            vec![TypePair::new("text", "text")]
        }

        fn convert(&self, _pair: &TypePair, value: &RawCell) -> Result<RawCell, CdmError> {
            Ok(value.clone())
        }
    }

    #[test]
    fn cdc_030_an_unparseable_declared_type_is_a_startup_error() {
        let core = Registry::builder()
            .register_codec(third_party("list<", "text"))
            .build()
            .unwrap();
        let error = CodecRegistry::from_registry(&core).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Config);
        assert!(error.to_string().contains("list<"), "{error}");
    }

    #[test]
    fn cdc_031_the_registry_lists_every_codec_and_the_pairs_it_serves() {
        let core = Registry::builder()
            .register_codec(third_party("text", "blob"))
            .build()
            .unwrap();
        let registry = CodecRegistry::from_registry(&core).unwrap();
        assert_eq!(registry.len(), 1);
        assert!(!registry.is_empty());
        assert_eq!(
            registry.descriptions(),
            vec![CodecDescription {
                codec: "ROT13".to_owned(),
                provider: "example-plugin".to_owned(),
                from: "text".to_owned(),
                to: "blob".to_owned(),
            }]
        );
        let entry = &registry.entries()[0];
        assert_eq!(entry.codec(), "ROT13");
        assert_eq!(entry.provider(), "example-plugin");
        assert_eq!(entry.origin(), &CqlTypeInfo::Text);
        assert_eq!(entry.target(), &CqlTypeInfo::Blob);
        assert!(format!("{entry:?}").contains("ROT13"));
    }

    #[test]
    fn cdc_030_an_empty_registry_resolves_nothing() {
        let registry = CodecRegistry::default();
        assert!(registry.is_empty());
        assert!(registry
            .converter(&CqlTypeInfo::Int, &CqlTypeInfo::Text)
            .is_none());
    }
}
