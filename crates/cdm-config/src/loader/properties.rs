//! A parser for Java `.properties` files (`CFG-011`).
//!
//! Java CDM's reference configuration, `cdm-detailed.properties`, separates key from value with
//! whitespace; Spark's own `--properties-file` uses the same format; and `java.util.Properties`
//! additionally accepts `=` and `:`. All three are accepted here, along with `#`/`!` comments,
//! backslash line continuations and the `\uXXXX`, `\n`, `\t`, `\r`, `\\` escapes.

/// One key/value pair, with the line it came from for diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The property name, exactly as written.
    pub key: String,
    /// The value, with escapes decoded.
    pub value: String,
    /// The one-based line the entry started on.
    pub line: usize,
}

/// Parses the contents of a `.properties` file.
///
/// Malformed input is not an error: a line that is only a key yields an empty value, exactly as
/// `java.util.Properties` does, and the empty value is then rejected by Tier 1 if the property
/// does not permit one (`CFG-027`).
///
/// ```
/// use cdm_config::loader::properties::parse;
///
/// let entries = parse("# comment\nspark.cdm.perfops.numParts   10000\na.b=c\n");
/// assert_eq!(entries.len(), 2);
/// assert_eq!(entries[0].key, "spark.cdm.perfops.numParts");
/// assert_eq!(entries[0].value, "10000");
/// ```
pub fn parse(text: &str) -> Vec<Entry> {
    let mut entries = Vec::new();
    let mut logical = String::new();
    let mut start_line = 0_usize;

    for (index, raw) in text.lines().enumerate() {
        let line = raw.trim_start();
        if logical.is_empty() {
            if line.is_empty() || line.starts_with('#') || line.starts_with('!') {
                continue;
            }
            start_line = index + 1;
        }

        if let Some(head) = continued(line) {
            logical.push_str(head);
            continue;
        }

        logical.push_str(line);
        if let Some(entry) = split_entry(&logical, start_line) {
            entries.push(entry);
        }
        logical.clear();
    }

    // A file whose last line ends in a continuation still yields its entry.
    if !logical.is_empty() {
        if let Some(entry) = split_entry(&logical, start_line) {
            entries.push(entry);
        }
    }

    entries
}

/// The content of a line that ends in an odd number of backslashes, i.e. a continuation.
fn continued(line: &str) -> Option<&str> {
    let trailing = line.len() - line.trim_end_matches('\\').len();
    if trailing % 2 == 1 {
        line.get(..line.len() - 1)
    } else {
        None
    }
}

/// Splits one logical line into a key and a value.
fn split_entry(logical: &str, line: usize) -> Option<Entry> {
    let mut chars = logical.char_indices().peekable();
    let mut key = String::new();
    let mut separator = None;

    while let Some((offset, ch)) = chars.next() {
        match ch {
            '\\' => {
                // An escaped separator belongs to the key.
                if let Some((_, escaped)) = chars.next() {
                    key.push(escaped);
                }
            }
            '=' | ':' => {
                separator = Some(offset + ch.len_utf8());
                break;
            }
            c if c.is_whitespace() => {
                separator = Some(offset);
                break;
            }
            c => key.push(c),
        }
    }

    if key.is_empty() {
        return None;
    }

    let rest = separator
        .and_then(|offset| logical.get(offset..))
        .unwrap_or_default()
        .trim_start();
    // `key = value` puts the `=` after the whitespace that ended the key.
    let rest = rest.strip_prefix(['=', ':']).map_or(rest, str::trim_start);

    Some(Entry {
        key,
        value: unescape(rest.trim_end()),
        line,
    })
}

/// Decodes the escapes `java.util.Properties` recognises in a value.
fn unescape(value: &str) -> String {
    if !value.contains('\\') {
        return value.to_owned();
    }
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('f') => out.push('\u{c}'),
            Some('u') => {
                let hex: String = chars.by_ref().take(4).collect();
                if let Some(decoded) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                    out.push(decoded);
                } else {
                    // An invalid escape is kept verbatim rather than swallowed, so the
                    // resulting value still shows the operator what they typed.
                    out.push_str("\\u");
                    out.push_str(&hex);
                }
            }
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
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

    fn pairs(text: &str) -> Vec<(String, String)> {
        parse(text)
            .into_iter()
            .map(|entry| (entry.key, entry.value))
            .collect()
    }

    #[test]
    fn cfg_011_the_java_reference_file_layout_parses() {
        // Exactly the shape of `src/resources/cdm-detailed.properties`.
        let text = "\
#===========================================================
# Origin
#===========================================================
spark.cdm.connect.origin.host                     localhost
spark.cdm.connect.origin.port                     9042
#spark.cdm.connect.origin.scb                     file://.../scb.zip
spark.cdm.schema.origin.keyspaceTable             origin_ks.tbl
";
        assert_eq!(
            pairs(text),
            [
                (
                    "spark.cdm.connect.origin.host".to_owned(),
                    "localhost".to_owned()
                ),
                (
                    "spark.cdm.connect.origin.port".to_owned(),
                    "9042".to_owned()
                ),
                (
                    "spark.cdm.schema.origin.keyspaceTable".to_owned(),
                    "origin_ks.tbl".to_owned()
                ),
            ]
        );
    }

    #[test]
    fn cfg_011_all_three_java_separators_are_accepted() {
        assert_eq!(
            pairs("a.b c\nd.e=f\ng.h:i\nj.k = l\n"),
            [
                ("a.b".to_owned(), "c".to_owned()),
                ("d.e".to_owned(), "f".to_owned()),
                ("g.h".to_owned(), "i".to_owned()),
                ("j.k".to_owned(), "l".to_owned()),
            ]
        );
    }

    #[test]
    fn cfg_011_comments_blank_lines_and_bare_keys_behave_as_java_does() {
        assert_eq!(
            pairs("# hash\n! bang\n\n   \nlonely.key\n"),
            [("lonely.key".to_owned(), String::new())]
        );
    }

    #[test]
    fn cfg_011_continuations_and_escapes_are_decoded() {
        assert_eq!(
            pairs("a.b  one, \\\n     two\n"),
            [("a.b".to_owned(), "one, two".to_owned())]
        );
        assert_eq!(
            pairs("a.b  x\\ty\\u0041z\n"),
            [("a.b".to_owned(), "x\ty\u{41}z".to_owned())]
        );
        // A malformed unicode escape survives verbatim rather than vanishing.
        assert_eq!(
            pairs("a.b  \\uZZZZ\n"),
            [("a.b".to_owned(), "\\uZZZZ".to_owned())]
        );
        // An unterminated continuation still yields its entry.
        assert_eq!(pairs("a.b  x\\"), [("a.b".to_owned(), "x".to_owned())]);
    }

    #[test]
    fn cfg_011_values_may_contain_separators() {
        assert_eq!(
            pairs("spark.cdm.connect.origin.scb  file:///tmp/scb.zip\n"),
            [(
                "spark.cdm.connect.origin.scb".to_owned(),
                "file:///tmp/scb.zip".to_owned()
            )]
        );
        assert_eq!(
            pairs("spark.cdm.filter.cassandra.whereCondition  a = 'b'\n"),
            [(
                "spark.cdm.filter.cassandra.whereCondition".to_owned(),
                "a = 'b'".to_owned()
            )]
        );
    }

    #[test]
    fn cfg_011_entries_remember_their_line_for_diagnostics() {
        let entries = parse("# c\n\na.b 1\nc.d 2\n");
        assert_eq!(entries[0].line, 3);
        assert_eq!(entries[1].line, 4);
    }
}
