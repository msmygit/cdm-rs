//! The `cdm_properties!` macro: one declaration, six artefacts (`CFG-001`, `CFG-002`).
//!
//! # Why a declarative macro and not a derive
//!
//! [`ADR-0005`] proposes a `CdmProperties` **derive** macro. A derive would need its own
//! proc-macro crate, and `ARCHITECTURE.md` §3 fixes the workspace at sixteen crates with a
//! documented dependency graph; adding a seventeenth for a compile-time convenience is a bigger
//! change to the architecture than it is worth. A declarative macro achieves the *decision* of
//! ADR-0005 — configuration defined exactly once — with no new crate and no proc-macro build
//! cost, at the price of a fixed attribute order. If the metadata surface ever outgrows
//! `macro_rules!`, swapping in a derive is a mechanical change: the field syntax below is
//! deliberately attribute-shaped so it can be lifted verbatim.
//!
//! [`ADR-0005`]: https://github.com/msmygit/cdm-rs/blob/main/docs/adr/0005-config-as-one-typed-model.md
//!
//! # What one invocation emits
//!
//! * the `struct`, with `serde`, `schemars` and `Clone`/`Debug`/`PartialEq` derives;
//! * `impl Default`, from the `= <expr>` on each field — the *only* place a default is written;
//! * `impl `[`Properties`](crate::meta::Properties), which yields the registry rows that in turn
//!   drive the `.properties` loader's alias table, `docs/generated/PROPERTIES.md`, `cdm config
//!   explain`, and the config-builder UI form descriptors.
//!
//! # Field syntax
//!
//! ```
//! # use cdm_config::cdm_properties;
//! cdm_properties! {
//!     /// Performance and operational tuning.
//!     pub struct Example {
//!         fields {
//!             /// Number of token-range splits the ring is divided into.
//!             ///
//!             /// Rule of thumb: `table_size / 10 MB`.
//!             #[cdm(legacy = ["spark.cdm.perfops.numParts"], unit = "parts")]
//!             pub num_parts: u64 = 5000,
//!         }
//!     }
//! }
//! assert_eq!(Example::default().num_parts, 5000);
//! ```
//!
//! Attributes are all optional but must appear in this order: `key`, `legacy`, `unit`,
//! `secret`, `default_note`, `example`, `stability`. `key` defaults to the field name and is
//! only needed when the canonical name cannot be a Rust identifier (`type`). Nested sections go
//! in a `sections { .. }` block and may rebind the `{side}` placeholder used in legacy aliases.

/// Declares a configuration struct together with its property metadata.
///
/// See the [module documentation](self) for the field syntax.
#[macro_export]
macro_rules! cdm_properties {
    (
        $(#[$smeta:meta])*
        $vis:vis struct $name:ident {
            $(fields {
                $(
                    $(#[doc = $fdoc:literal])+
                    #[cdm(
                        $(key = $fkey:literal $(,)?)?
                        $(legacy = [$($flegacy:literal),* $(,)?] $(,)?)?
                        $(unit = $funit:literal $(,)?)?
                        $(secret = $fsecret:literal $(,)?)?
                        $(default_note = $fnote:literal $(,)?)?
                        $(example = $fexample:literal $(,)?)?
                        $(stability = $fstab:ident $(,)?)?
                    )]
                    $fvis:vis $field:ident : $fty:ty = $fdefault:expr
                ),* $(,)?
            })?
            $(sections {
                $(
                    $(#[doc = $sdoc:literal])+
                    #[cdm(
                        $(key = $skey:literal $(,)?)?
                        $(side = $sside:literal $(,)?)?
                    )]
                    $svis:vis $sfield:ident : $sty:ty
                ),* $(,)?
            })?
        }
    ) => {
        $(#[$smeta])*
        #[derive(
            ::core::fmt::Debug,
            ::core::clone::Clone,
            ::core::cmp::PartialEq,
            ::serde::Serialize,
            ::serde::Deserialize,
            ::schemars::JsonSchema,
        )]
        #[serde(default, deny_unknown_fields)]
        #[allow(clippy::struct_excessive_bools)]
        $vis struct $name {
            $($(
                $(#[doc = $fdoc])+
                $(#[serde(rename = $fkey)])?
                $fvis $field: $fty,
            )*)?
            $($(
                $(#[doc = $sdoc])+
                $(#[serde(rename = $skey)])?
                $svis $sfield: $sty,
            )*)?
        }

        impl ::core::default::Default for $name {
            fn default() -> Self {
                Self {
                    $($( $field: $fdefault, )*)?
                    $($( $sfield: <$sty as ::core::default::Default>::default(), )*)?
                }
            }
        }

        impl $crate::meta::Properties for $name {
            fn properties(
                ctx: &$crate::meta::MetaContext,
            ) -> ::std::vec::Vec<$crate::meta::PropertyMeta> {
                #[allow(unused_mut)]
                let mut out: ::std::vec::Vec<$crate::meta::PropertyMeta> =
                    ::std::vec::Vec::new();
                #[allow(unused_variables)]
                let defaults = <Self as ::core::default::Default>::default();
                $($(
                    out.push($crate::meta::PropertyMeta::build(
                        ctx,
                        $crate::meta::PropertySpec {
                            key: $crate::__cdm_key!($field $(, $fkey)?),
                            legacy: &[$($($flegacy,)*)?],
                            kind: <$fty as $crate::meta::PropertyValue>::kind(),
                            optional: <$fty as $crate::meta::PropertyValue>::optional(),
                            default: $crate::meta::PropertyValue::display_value(
                                &defaults.$field,
                            ),
                            unit: $crate::__cdm_opt!($($funit)?),
                            secret: $crate::__cdm_flag!($($fsecret)?),
                            default_note: $crate::__cdm_opt!($($fnote)?),
                            example: $crate::__cdm_opt!($($fexample)?),
                            stability: $crate::__cdm_stability!($($fstab)?),
                            doc: &[$($fdoc,)+],
                        },
                    ));
                )*)?
                $($(
                    out.extend(<$sty as $crate::meta::Properties>::properties(
                        &ctx.child(
                            $crate::__cdm_key!($sfield $(, $skey)?),
                            $crate::__cdm_opt!($($sside)?),
                        ),
                    ));
                )*)?
                out
            }
        }
    };
}

/// The canonical key of a field: the explicit `key = ".."` if given, else the field name.
#[doc(hidden)]
#[macro_export]
macro_rules! __cdm_key {
    ($field:ident) => {
        ::core::stringify!($field)
    };
    ($field:ident, $key:literal) => {
        $key
    };
}

/// `Some(v)` when an optional attribute was given, `None` otherwise.
#[doc(hidden)]
#[macro_export]
macro_rules! __cdm_opt {
    () => {
        ::core::option::Option::None
    };
    ($value:literal) => {
        ::core::option::Option::Some($value)
    };
}

/// An optional boolean attribute, defaulting to `false`.
#[doc(hidden)]
#[macro_export]
macro_rules! __cdm_flag {
    () => {
        false
    };
    ($value:literal) => {
        $value
    };
}

/// An optional stability marker, defaulting to [`Stability::Stable`](crate::meta::Stability).
#[doc(hidden)]
#[macro_export]
macro_rules! __cdm_stability {
    () => {
        $crate::meta::Stability::Stable
    };
    (stable) => {
        $crate::meta::Stability::Stable
    };
    (experimental) => {
        $crate::meta::Stability::Experimental
    };
    (deprecated) => {
        $crate::meta::Stability::Deprecated
    };
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
    use crate::meta::{MetaContext, Properties, PropertyKind, Stability};

    crate::cdm_properties! {
        /// A nested section, instantiated once per side.
        pub(crate) struct Inner {
            fields {
                /// Whether the thing is on.
                #[cdm(legacy = ["spark.cdm.connect.{side}.tls.enabled"])]
                pub(crate) enabled: bool = false,

                /// The keystore type.
                #[cdm(
                    key = "type",
                    legacy = ["spark.cdm.connect.{side}.tls.trustStore.type"],
                    stability = experimental,
                )]
                pub(crate) store_type: String = "JKS".to_owned(),
            }
        }
    }

    crate::cdm_properties! {
        /// The outer struct.
        pub(crate) struct Outer {
            fields {
                /// A credential.
                #[cdm(legacy = ["spark.cdm.x.password"], secret = true, stability = deprecated)]
                pub(crate) password: String = "cassandra".to_owned(),

                /// A tuned number.
                #[cdm(unit = "rows/s", default_note = "num_cpus", example = "8")]
                pub(crate) workers: Option<u32> = None,
            }
            sections {
                /// The origin side.
                #[cdm(side = "origin")]
                pub(crate) origin: Inner,

                /// The target side.
                #[cdm(side = "target")]
                pub(crate) target: Inner
            }
        }
    }

    #[test]
    fn cfg_001_one_declaration_yields_struct_defaults_and_registry_rows() {
        let defaults = Outer::default();
        assert_eq!(defaults.password, "cassandra");
        assert_eq!(defaults.workers, None);
        assert!(!defaults.origin.enabled);
        assert_eq!(defaults.target.store_type, "JKS");

        let rows = Outer::properties(&MetaContext::root());
        let canonical: Vec<&str> = rows.iter().map(|r| r.canonical.as_str()).collect();
        assert_eq!(
            canonical,
            [
                "password",
                "workers",
                "origin.enabled",
                "origin.type",
                "target.enabled",
                "target.type",
            ]
        );
    }

    #[test]
    fn cfg_002_every_row_carries_alias_default_unit_secrecy_and_stability() {
        let rows = Outer::properties(&MetaContext::root());
        let password = &rows[0];
        assert!(password.secret);
        assert_eq!(password.stability, Stability::Deprecated);
        assert_eq!(password.default_value.as_deref(), Some("cassandra"));
        assert_eq!(password.displayed_default().as_deref(), Some("***"));
        assert_eq!(password.summary, "A credential.");

        let workers = &rows[1];
        assert!(workers.optional);
        assert_eq!(workers.unit, Some("rows/s"));
        assert_eq!(workers.default_note, Some("num_cpus"));
        assert_eq!(workers.example, Some("8"));
        assert_eq!(workers.kind, PropertyKind::Integer);

        // One `Inner` declaration produced both sides' legacy aliases.
        assert_eq!(rows[2].legacy, ["spark.cdm.connect.origin.tls.enabled"]);
        assert_eq!(rows[4].legacy, ["spark.cdm.connect.target.tls.enabled"]);
        assert_eq!(rows[3].stability, Stability::Experimental);
    }

    #[test]
    fn cfg_001_the_serde_representation_follows_the_canonical_key() {
        let json = serde_json::to_value(Inner::default()).unwrap();
        assert_eq!(json, serde_json::json!({"enabled": false, "type": "JKS"}));
        let parsed: Inner = serde_json::from_value(serde_json::json!({"type": "PEM"})).unwrap();
        assert_eq!(parsed.store_type, "PEM");
        assert!(!parsed.enabled);
        // Unknown keys are refused so that a typo cannot be silently ignored (CFG-011).
        assert!(serde_json::from_value::<Inner>(serde_json::json!({"nope": 1})).is_err());
    }
}
