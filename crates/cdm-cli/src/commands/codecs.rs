//! `cdm codecs` — what conversions exist, and which type pairs they serve (`CDC-031`).
//!
//! The question this answers is asked before a migration, not during one: "my origin column is a
//! `text` and my target column is a `timestamp` — will cdm-rs handle that, and what do I have to
//! turn on?" Java's answer was to read `Codecset.java`. This one is a command.
//!
//! Every built-in codec is listed, whether or not the configuration in front of the operator
//! enables it, because the list is the *catalogue* — a codec absent from it cannot be requested at
//! all, which is a different answer from "you did not ask for it". `transform.codecs` then selects
//! from the catalogue, and `cdm config explain transform.codecs` says what a given run selected.

use std::io::Write;

use cdm_codec::{CodecDescription, CodecRegistry, Codecset, TimestampFormat};
use cdm_core::CdmError;
use serde::Serialize;

use crate::output::Report;

/// The catalogue of registered conversions (`CDC-031`).
#[derive(Debug, Serialize)]
pub struct CodecsReport {
    /// The named codec sets, as `transform.codecs` spells them.
    pub codecsets: Vec<&'static str>,
    /// Every registered `(from, to)` pair, in registration order.
    pub conversions: Vec<CodecDescription>,
}

impl Report for CodecsReport {
    fn render_human(&self, out: &mut dyn Write) -> std::io::Result<()> {
        writeln!(
            out,
            "{} codec set(s), {} conversion(s). Enable them with `transform.codecs`.\n",
            self.codecsets.len(),
            self.conversions.len()
        )?;

        // Widths from the data rather than a guess: a UDT or a `vector<float, 1536>` is long, and
        // a column that wraps is a column that cannot be read.
        let width = |f: fn(&CodecDescription) -> &str| {
            self.conversions
                .iter()
                .map(|c| f(c).chars().count())
                .max()
                .unwrap_or(0)
        };
        let codec_width = width(|c| c.codec.as_str());
        let from_width = width(|c| c.from.as_str());

        for conversion in &self.conversions {
            writeln!(
                out,
                "  {:codec_width$}  {:from_width$} -> {}  [{}]",
                conversion.codec, conversion.from, conversion.to, conversion.provider
            )?;
        }
        Ok(())
    }
}

/// Lists every registered codec and the type pairs it serves (`CDC-031`).
///
/// # Errors
///
/// [`cdm_core::ErrorKind::Config`] if the registry rejects the built-in set, which would be a
/// defect rather than a user error: the same construction runs on every startup.
pub fn list() -> Result<CodecsReport, CdmError> {
    // The catalogue cannot be one registry. `TIMESTAMP_STRING_MILLIS` and `TIMESTAMP_STRING_FORMAT`
    // both claim `timestamp -> text`, and `PLG-010` makes a doubly-claimed pair a startup error —
    // correctly, because a run in which both are enabled has no defined answer for a timestamp
    // column. They are therefore registered in two passes and their descriptions concatenated.
    // Listing both is right even though no single run can hold both: this is the catalogue of what
    // may be asked for, and a codec absent from it cannot be asked for at all.
    let exclusive = Codecset::TimestampStringFormat;
    let mut enabled: Vec<Codecset> = Codecset::ALL
        .into_iter()
        .filter(|codec| *codec != exclusive)
        .collect();

    let mut conversions = CodecRegistry::with_builtins(&enabled, None)?.descriptions();

    // `TIMESTAMP_STRING_FORMAT` is also the one codec whose registration needs a parameter — the
    // pattern from `transform.codecs.timestamp_format` (`CDC-021`). A placeholder is supplied
    // purely so the codec can be *listed*; nothing here converts a value, and the pattern is never
    // shown, so its content cannot mislead.
    enabled.clear();
    enabled.push(exclusive);
    let format = TimestampFormat::new("yyyy-MM-dd HH:mm:ss", "UTC")?;
    // The always-registered `BIGINT_BIGINTEGER` appears in both registries (`CDC-020`); listing it
    // twice would read as two codecs claiming the same pair, which is the one thing `PLG-010` says
    // cannot happen.
    let second_pass: Vec<_> = CodecRegistry::with_builtins(&enabled, Some(format))?
        .descriptions()
        .into_iter()
        .filter(|second| {
            !conversions
                .iter()
                .any(|first| first.codec == second.codec && first.from == second.from)
        })
        .collect();
    conversions.extend(second_pass);

    Ok(CodecsReport {
        codecsets: Codecset::ALL.into_iter().map(Codecset::name).collect(),
        conversions,
    })
}

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
    fn cdc_031_every_named_codecset_appears_in_the_catalogue() {
        let report = list().unwrap();
        for codec in Codecset::ALL {
            assert!(
                report.codecsets.contains(&codec.name()),
                "{} is configurable but absent from `cdm codecs`, which is how an operator \
                 concludes cdm-rs cannot do a conversion it can do",
                codec.name()
            );
        }
    }

    #[test]
    fn cdc_031_the_two_mutually_exclusive_timestamp_codecs_are_both_listed() {
        // They cannot both be enabled — `PLG-010` refuses the doubly-claimed `timestamp -> text`
        // pair — but an operator choosing between them has to be able to see that both exist. A
        // catalogue that silently dropped one would read as "cdm-rs cannot do that".
        let report = list().unwrap();
        let named = |codec: &str| report.conversions.iter().any(|c| c.codec == codec);
        assert!(named("TIMESTAMP_STRING_MILLIS"));
        assert!(named("TIMESTAMP_STRING_FORMAT"));
    }

    #[test]
    fn cdc_031_no_type_pair_is_listed_twice_for_one_codec() {
        // `BIGINT_BIGINTEGER` is registered whether or not it is asked for (`CDC-020`), so it turns
        // up in both passes; a duplicate would read as the conflict `PLG-010` says cannot exist.
        let report = list().unwrap();
        let mut seen = std::collections::BTreeSet::new();
        for conversion in &report.conversions {
            assert!(
                seen.insert((
                    conversion.codec.clone(),
                    conversion.from.clone(),
                    conversion.to.clone()
                )),
                "{conversion:?} is listed twice"
            );
        }
    }

    #[test]
    fn cdc_031_each_conversion_names_both_types_and_its_provider() {
        let report = list().unwrap();
        assert!(!report.conversions.is_empty());
        for conversion in &report.conversions {
            assert!(!conversion.from.is_empty(), "{conversion:?}");
            assert!(!conversion.to.is_empty(), "{conversion:?}");
            assert!(!conversion.provider.is_empty(), "{conversion:?}");
        }
    }

    #[test]
    fn cdc_031_the_human_rendering_lists_the_type_pairs() {
        let mut buf = Vec::new();
        list().unwrap().render_human(&mut buf).unwrap();
        let text = String::from_utf8(buf).unwrap();
        assert!(text.contains("->"), "{text}");
        assert!(text.contains("transform.codecs"), "{text}");
    }
}
