//! CQL identifier quoting and folding (`SCH-002`).
//!
//! An unquoted CQL identifier is case-insensitive and folds to lower case; a quoted one is taken
//! literally, and an embedded `"` is written `""`. Getting this wrong is not a cosmetic problem:
//! a table with a column named `Data` and another named `data` is legal, and a statement that
//! quotes neither reads the wrong one.
//!
//! The rules here mirror the Java driver's `CqlIdentifier`, which Java CDM uses through
//! `CqlTable.formatName` / `unFormatName`:
//!
//! * **internal → CQL** ([`format()`]): quote unless the name is already `[a-z][a-z0-9_]*` and is
//!   not a reserved keyword; double any embedded `"`;
//! * **CQL → internal** ([`unformat`]): strip the quotes and undouble; an unquoted name is
//!   returned unchanged, exactly as Java does, with the case-insensitivity of an unquoted
//!   identifier applied as the [`fold`] fallback in [`super::introspect`];
//! * one passthrough Java has and cdm-rs keeps for parity: a name that is *already* quoted is
//!   returned unchanged.
//!
//! A function form such as `TTL(data)` or `WRITETIME(data)` survives formatting too, so the
//! virtual projection columns of `SCH-007` can be formatted alongside real ones — but **its
//! argument is formatted**, where Java's `formatName` returns the whole thing untouched
//! (`return name; // function`). That is divergence 42: Java quotes the column in the projection
//! list and leaves it bare inside the function, so a reserved-word column produces
//! `SELECT col1,"order",TTL(order)` and the server rejects the statement outright.

/// The reserved words of the CQL grammar, which must be quoted to be used as identifiers.
///
/// Taken from Apache Cassandra's `Cql.g` reserved-keyword list (5.0), which is a superset of
/// every earlier version's, so a name quoted here is accepted everywhere. Over-quoting is safe;
/// under-quoting is a syntax error at run time.
pub const RESERVED_KEYWORDS: &[&str] = &[
    "add",
    "allow",
    "alter",
    "and",
    "apply",
    "asc",
    "authorize",
    "batch",
    "begin",
    "by",
    "columnfamily",
    "create",
    "default",
    "delete",
    "desc",
    "describe",
    "drop",
    "entries",
    "execute",
    "from",
    "full",
    "grant",
    "if",
    "in",
    "index",
    "infinity",
    "insert",
    "into",
    "is",
    "keyspace",
    "limit",
    "materialized",
    "modify",
    "nan",
    "norecursive",
    "not",
    "null",
    "of",
    "on",
    "or",
    "order",
    "primary",
    "rename",
    "replace",
    "revoke",
    "schema",
    "select",
    "set",
    "table",
    "to",
    "token",
    "truncate",
    "unlogged",
    "unset",
    "update",
    "use",
    "using",
    "view",
    "where",
    "with",
];

/// Whether `name` is a reserved word and therefore always needs quoting.
pub fn is_reserved(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    RESERVED_KEYWORDS.binary_search(&lower.as_str()).is_ok()
}

/// Whether `name` can be written without quotes: `[a-z][a-z0-9_]*` and not reserved.
pub fn needs_quoting(name: &str) -> bool {
    if name.is_empty() {
        return true;
    }
    let mut chars = name.chars();
    let first_is_plain = chars
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c == '_');
    let rest_is_plain = chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    !(first_is_plain && rest_is_plain) || is_reserved(name)
}

/// Whether `name` is already written as a quoted CQL identifier.
pub fn is_quoted(name: &str) -> bool {
    name.len() >= 2
        && name.starts_with('"')
        && name.ends_with('"')
        && !name.chars().any(char::is_whitespace)
}

/// Whether `name` is a function call such as `TTL(data)` rather than a plain identifier.
///
/// Java's `formatName` passes these through untouched, which is what lets `SCH-007`'s virtual
/// `TTL(col)` / `WRITETIME(col)` projection columns be formatted alongside real ones.
pub fn is_function_form(name: &str) -> bool {
    let Some(open) = name.find('(') else {
        return false;
    };
    if !name.ends_with(')') {
        return false;
    }
    let head = name.get(..open).unwrap_or_default();
    !head.is_empty() && head.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Writes an internal identifier as CQL, quoting it only when it must be (`SCH-002`).
///
/// ```
/// use cdm_cql::schema::identifier::format;
///
/// assert_eq!(format("data"), "data");
/// assert_eq!(format("Data"), "\"Data\"");
/// assert_eq!(format("token"), "\"token\"");
/// assert_eq!(format("we\"ird"), "\"we\"\"ird\"");
/// assert_eq!(format("TTL(data)"), "TTL(data)");
/// // SCH-007: the argument of a virtual column is an identifier too, and obeys the same rule.
/// assert_eq!(format("TTL(order)"), "TTL(\"order\")");
/// assert_eq!(format("WRITETIME(Data)"), "WRITETIME(\"Data\")");
/// ```
pub fn format(name: &str) -> String {
    if name.is_empty() || is_quoted(name) {
        return name.to_owned();
    }
    // SCH-007: a function form is not opaque. `TTL(col)` is a function *applied to an identifier*,
    // and the identifier inside obeys `SCH-002` exactly as it does in the projection list — so
    // `TTL(order)` is a syntax error where `TTL("order")` is what was meant. Formatting the
    // argument here rather than at each call site is what keeps the two spellings of the same
    // column from disagreeing: the projection already quotes it, and only this path did not.
    if let Some((function, argument)) = split_function_form(name) {
        // Idempotent: callers reach this both ways. `WritetimeTtlPlan` builds `TTL(col)` from an
        // internal name, while an `OriginProjection` may be handed `WRITETIME("My Col")` already
        // formatted. `is_quoted` cannot be the test — it is deliberately strict for Java parity
        // and rejects a quoted name containing whitespace, which would double-quote exactly the
        // names that most need quoting.
        let formatted = if already_quoted(argument) {
            argument.to_owned()
        } else {
            format(argument)
        };
        return format!("{function}({formatted})");
    }
    if needs_quoting(name) {
        quote(name)
    } else {
        name.to_owned()
    }
}

/// Whether `name` is already written with surrounding quotes, whitespace and all.
///
/// Deliberately more permissive than [`is_quoted`], which mirrors Java's `^"[^\s]*"$` and so does
/// not recognise `"My Col"`. That strictness is right where it is used; here it would mean
/// re-quoting a name that is already quoted.
fn already_quoted(name: &str) -> bool {
    name.len() >= 2 && name.starts_with('"') && name.ends_with('"')
}

/// Splits `TTL(col)` into `("TTL", "col")`, or `None` if `name` is not a function form.
fn split_function_form(name: &str) -> Option<(&str, &str)> {
    let open = name.find('(')?;
    if !name.ends_with(')') {
        return None;
    }
    let head = name.get(..open)?;
    if head.is_empty() || !head.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    let argument = name.get(open + 1..name.len() - 1)?;
    Some((head, argument))
}

/// Writes an internal identifier as a quoted CQL identifier, always.
pub fn quote(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// Reads a CQL identifier back to its internal form (`SCH-002`).
///
/// ```
/// use cdm_cql::schema::identifier::unformat;
///
/// assert_eq!(unformat("\"Data\""), "Data");
/// assert_eq!(unformat("Data"), "Data");
/// assert_eq!(unformat("\"we\"\"ird\""), "we\"ird");
/// ```
pub fn unformat(name: &str) -> String {
    if name.is_empty() {
        return String::new();
    }
    if is_quoted(name) {
        return name
            .get(1..name.len() - 1)
            .unwrap_or_default()
            .replace("\"\"", "\"");
    }
    if name.contains('"') || name.chars().any(char::is_whitespace) {
        // Not a well-formed quoted name and not a bare one — a value like `"a b"`. Strip the
        // outer quotes if they are there, and otherwise leave it alone rather than mangling it.
        let trimmed = name.trim();
        if trimmed.len() >= 2 && trimmed.starts_with('"') && trimmed.ends_with('"') {
            return trimmed
                .get(1..trimmed.len() - 1)
                .unwrap_or_default()
                .replace("\"\"", "\"");
        }
        return name.to_owned();
    }
    // Java parity: an unquoted name is returned unchanged, *not* folded. `system_schema` stores
    // internal names, and folding here would make a table created as `"MyTable"` unfindable.
    name.to_owned()
}

/// The name an **unquoted** CQL identifier resolves to: the same name, folded to lower case.
///
/// This is the half of `SCH-002` that [`unformat`] deliberately does not do. It is applied as a
/// fallback when an exact lookup finds nothing, so that an operator who writes
/// `schema.origin.keyspace_table = MY_KS.MyTable` — which cqlsh would resolve to `my_ks.mytable` —
/// gets the table rather than "no such table". An exact match always wins, so a cluster holding
/// both `MyTable` and `mytable` is never confused.
pub fn fold(name: &str) -> String {
    name.to_ascii_lowercase()
}

/// A `keyspace.table` reference, each part quoted as needed (`SCH-002`).
///
/// ```
/// use cdm_cql::schema::identifier::qualified;
///
/// assert_eq!(qualified("ks", "tbl"), "ks.tbl");
/// assert_eq!(qualified("My_KS", "select"), "\"My_KS\".\"select\"");
/// ```
pub fn qualified(keyspace: &str, table: &str) -> String {
    format!("{}.{}", format(keyspace), format(table))
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
    #[test]
    fn sch_007_a_virtual_column_quotes_its_argument_like_any_other_identifier() {
        // The defect: `format` treated a function form as opaque, so a reserved-word column was
        // quoted in the projection list and left bare inside TTL()/WRITETIME(). The server saw
        // `SELECT col1,"order",TTL(order)` and rejected the statement outright.
        assert_eq!(format("TTL(order)"), "TTL(\"order\")");
        assert_eq!(format("WRITETIME(order)"), "WRITETIME(\"order\")");
        // Case-sensitive and awkward names go the same way.
        assert_eq!(format("TTL(Data)"), "TTL(\"Data\")");
        assert_eq!(format("TTL(we\"ird)"), "TTL(\"we\"\"ird\")");
        // A name that needs no quoting is still left alone, so nothing that worked changes.
        assert_eq!(format("TTL(data)"), "TTL(data)");
        assert_eq!(format("WRITETIME(data)"), "WRITETIME(data)");
        // An argument already quoted is not quoted twice — including one with whitespace, which
        // `is_quoted` deliberately does not recognise.
        assert_eq!(format("TTL(\"order\")"), "TTL(\"order\")");
        assert_eq!(format("WRITETIME(\"My Col\")"), "WRITETIME(\"My Col\")");
        // And an unquoted name with whitespace is still quoted once.
        assert_eq!(format("WRITETIME(My Col)"), "WRITETIME(\"My Col\")");
    }

    #[test]
    fn sch_002_a_name_that_merely_contains_a_bracket_is_not_a_function_form() {
        // `split_function_form` must not turn an odd column name into a function call.
        assert_eq!(split_function_form("(x)"), None);
        assert_eq!(split_function_form("we(ird"), None);
        assert_eq!(split_function_form("a b(c)"), None);
        assert_eq!(split_function_form("TTL(col)"), Some(("TTL", "col")));
    }

    use super::*;

    #[test]
    fn sch_002_a_plain_lowercase_name_is_not_quoted() {
        assert_eq!(format("data"), "data");
        assert_eq!(format("data_2"), "data_2");
        assert_eq!(format("_hidden"), "_hidden");
        assert!(!needs_quoting("data"));
    }

    #[test]
    fn sch_002_a_mixed_case_name_is_quoted_and_survives_a_round_trip() {
        assert_eq!(format("Data"), "\"Data\"");
        assert_eq!(format("myColumn"), "\"myColumn\"");
        assert_eq!(unformat(&format("myColumn")), "myColumn");
    }

    #[test]
    fn sch_002_a_reserved_keyword_is_quoted() {
        // SIT `05_reserved_keyword` is exactly this case.
        for keyword in ["token", "select", "from", "where", "table", "order", "if"] {
            assert_eq!(format(keyword), format!("\"{keyword}\""), "{keyword}");
            assert!(is_reserved(keyword));
            assert!(is_reserved(&keyword.to_uppercase()));
        }
        assert!(!is_reserved("data"));
    }

    #[test]
    fn sch_002_the_keyword_list_is_sorted_so_the_binary_search_is_valid() {
        let mut sorted = RESERVED_KEYWORDS.to_vec();
        sorted.sort_unstable();
        assert_eq!(sorted, RESERVED_KEYWORDS.to_vec());
        sorted.dedup();
        assert_eq!(sorted.len(), RESERVED_KEYWORDS.len(), "duplicate keyword");
    }

    #[test]
    fn sch_002_an_embedded_quote_is_doubled() {
        assert_eq!(format("we\"ird"), "\"we\"\"ird\"");
        assert_eq!(unformat("\"we\"\"ird\""), "we\"ird");
        assert_eq!(unformat(&format("we\"ird")), "we\"ird");
    }

    #[test]
    fn sch_002_special_characters_force_quoting() {
        for name in ["col-1", "col 1", "col.1", "über", "1st", "COL$"] {
            assert!(needs_quoting(name), "{name}");
            assert!(format(name).starts_with('"'), "{name}");
        }
    }

    #[test]
    fn sch_002_an_already_quoted_name_is_left_alone() {
        assert_eq!(format("\"Data\""), "\"Data\"");
        assert_eq!(format("\"data\""), "\"data\"");
    }

    #[test]
    fn sch_002_a_function_form_is_left_alone() {
        // SCH-007's virtual columns pass through the same formatter as real ones.
        assert_eq!(format("TTL(data)"), "TTL(data)");
        assert_eq!(format("WRITETIME(data)"), "WRITETIME(data)");
        assert!(is_function_form("ttl(x)"));
        assert!(!is_function_form("(x)"));
        assert!(!is_function_form("ttl(x"));
        assert!(!is_function_form("my col(x)"));
    }

    #[test]
    fn sch_002_an_unquoted_name_is_read_back_unchanged_as_java_does() {
        // Java CDM's `unFormatName` returns an unquoted name verbatim, and `system_schema` stores
        // internal names — so folding here would make `"Reserved_Words"` unfindable.
        assert_eq!(unformat("DATA"), "DATA");
        assert_eq!(unformat("Reserved_Words"), "Reserved_Words");
        assert_eq!(unformat("data"), "data");
    }

    #[test]
    fn sch_002_folding_is_available_for_the_unquoted_cql_rule() {
        assert_eq!(fold("MyTable"), "mytable");
        assert_eq!(fold("mytable"), "mytable");
    }

    #[test]
    fn sch_002_a_qualified_name_quotes_each_part_independently() {
        assert_eq!(qualified("ks", "tbl"), "ks.tbl");
        assert_eq!(qualified("My_KS", "tbl"), "\"My_KS\".tbl");
        assert_eq!(qualified("ks", "select"), "ks.\"select\"");
        assert_eq!(qualified("My_KS", "select"), "\"My_KS\".\"select\"");
    }

    #[test]
    fn sch_002_the_empty_name_is_handled_rather_than_panicking() {
        assert_eq!(format(""), "");
        assert_eq!(unformat(""), "");
        assert!(needs_quoting(""));
        assert!(!is_quoted("\""));
    }

    #[test]
    fn sch_002_a_name_with_spaces_round_trips() {
        let internal = "two words";
        assert_eq!(format(internal), "\"two words\"");
        assert_eq!(unformat("\"two words\""), "two words");
    }

    #[test]
    fn sch_002_quote_always_quotes() {
        assert_eq!(quote("data"), "\"data\"");
        assert_eq!(quote("a\"b"), "\"a\"\"b\"");
    }
}
