//! Java formatting semantics: `DecimalFormat` for doubles (`CDC-020`) and
//! `SimpleDateFormat`/`DateTimeFormatter` patterns for timestamps (`CDC-022`).
//!
//! Both exist because the strings cdm-rs writes into a `text` column have to be byte-identical to
//! the ones Java CDM writes. A migration that switches tools mid-flight, or a validate run that
//! compares a Java-migrated table against a cdm-rs-migrated one, would otherwise report
//! mismatches that are pure formatting.

use std::fmt::Write as _;
use std::str::FromStr;

use bigdecimal::{BigDecimal, RoundingMode};
use cdm_core::{CdmError, ErrorKind};
use chrono::{NaiveDate, NaiveDateTime, Timelike};

/// The `DecimalFormat` pattern Java CDM hard-codes for `double` → `text`.
/// *(Java `DOUBLE_StringCodec.DOUBLE_FORMAT`.)*
pub const DOUBLE_FORMAT: &str = "0.#########";

/// The maximum number of fraction digits `0.#########` admits.
const DOUBLE_MAX_FRACTION_DIGITS: i64 = 9;

/// Formats a `double` exactly as Java CDM's `DOUBLE_StringCodec` does (`CDC-020`).
///
/// Java builds `new DecimalFormat("0.#########")`, calls `setGroupingUsed(false)` and
/// `setRoundingMode(RoundingMode.FLOOR)`, so the contract is:
///
/// * at most nine fraction digits, rounded **towards negative infinity**;
/// * trailing fraction zeros dropped, but always at least one integer digit;
/// * no grouping separators;
/// * **never** scientific notation — which is the reason the codec exists at all, since
///   `Double.toString` would emit `2.14748364707E10`.
///
/// The digits fed to the formatter are the shortest decimal that round-trips to the same `double`
/// — `DecimalFormat` formats `FloatingDecimal`'s digits, not the exact binary value — which is
/// also what Rust's `{}` produces, so the two agree digit for digit.
///
/// ```
/// use cdm_codec::format_double_java;
///
/// assert_eq!(format_double_java(21_474_836_470.7), "21474836470.7");
/// assert_eq!(format_double_java(1.0), "1");
/// assert_eq!(format_double_java(1e20), "100000000000000000000");
/// ```
pub fn format_double_java(value: f64) -> String {
    // DecimalFormat delegates these three to DecimalFormatSymbols. The symbols are locale
    // dependent in Java; cdm-rs pins the `Locale.US` spellings, because a locale-dependent
    // on-disk representation is not a contract anyone can migrate against.
    if value.is_nan() {
        return "NaN".to_owned();
    }
    if value.is_infinite() {
        return if value < 0.0 { "-∞" } else { "∞" }.to_owned();
    }

    // Rust's `{}` for f64 is the shortest round-tripping decimal, in positional notation.
    let shortest = format!("{value}");
    let Ok(decimal) = BigDecimal::from_str(&shortest) else {
        return shortest;
    };
    let (_, scale) = decimal.as_bigint_and_exponent();
    let rounded = if scale > DOUBLE_MAX_FRACTION_DIGITS {
        decimal.with_scale_round(DOUBLE_MAX_FRACTION_DIGITS, RoundingMode::Floor)
    } else if scale < 0 {
        // A negative scale would render as `1E+20`; renormalising to scale 0 keeps it positional.
        decimal.with_scale_round(0, RoundingMode::Floor)
    } else {
        decimal
    };

    let mut text = plain_string(&rounded);
    if text.contains('.') {
        text = text.trim_end_matches('0').trim_end_matches('.').to_owned();
    }
    // BigDecimal has no negative zero, but `DecimalFormat` prints one.
    if value.is_sign_negative() && !text.starts_with('-') {
        text.insert(0, '-');
    }
    text
}

/// Renders a decimal in plain positional notation, never exponential.
///
/// `BigDecimal`'s own `Display` switches to `1E-7` once the exponent is large enough, which is
/// exactly what `DecimalFormat` must never emit.
fn plain_string(value: &BigDecimal) -> String {
    let (unscaled, scale) = value.as_bigint_and_exponent();
    let negative = unscaled.sign() == num_bigint::Sign::Minus;
    let digits = unscaled.magnitude().to_string();
    let mut out = String::new();
    if negative {
        out.push('-');
    }
    if scale <= 0 {
        out.push_str(&digits);
        for _ in 0..-scale {
            out.push('0');
        }
        return out;
    }
    let scale = usize::try_from(scale).unwrap_or(usize::MAX);
    if digits.len() > scale {
        let split = digits.len() - scale;
        out.push_str(digits.get(..split).unwrap_or_default());
        out.push('.');
        out.push_str(digits.get(split..).unwrap_or_default());
    } else {
        out.push_str("0.");
        for _ in digits.len()..scale {
            out.push('0');
        }
        out.push_str(&digits);
    }
    out
}

/// Parses a `double` from text the way `Double.parseDouble` does.
///
/// # Errors
///
/// Returns [`ErrorKind::TypeConversion`] when the text is not a number. Java throws
/// `NumberFormatException` here, which CDM surfaces as a record-level failure.
pub fn parse_double_java(text: &str) -> Result<f64, CdmError> {
    text.trim().parse::<f64>().map_err(|e| {
        CdmError::new(
            ErrorKind::TypeConversion,
            format!("`{text}` is not a double: {e}"),
        )
    })
}

/// One field of a translated Java date pattern.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Field {
    /// Literal text, from a quoted section or from an unquoted non-letter run.
    Literal(String),
    /// `y` — year. Width 2 is Java's reduced-value form, based at 2000.
    Year(usize),
    /// `M` — month of year, numeric.
    Month(usize),
    /// `d` — day of month.
    Day(usize),
    /// `H` — hour of day, 0–23.
    Hour24(usize),
    /// `h` — clock hour of am/pm, 1–12.
    Hour12(usize),
    /// `m` — minute of hour.
    Minute(usize),
    /// `s` — second of minute.
    Second(usize),
    /// `S` — fraction of second, `width` digits.
    Fraction(usize),
    /// `a` — am/pm marker.
    AmPm,
}

/// A Java `SimpleDateFormat`/`DateTimeFormatter` pattern, translated for use in Rust (`CDC-022`).
///
/// Patterns are accepted **verbatim**, so `transform.codecs.timestamp_format = yyyyMMddHHmmss`
/// copied out of a Java CDM properties file keeps working. The supported pattern letters are
/// `y M d H h m s S a`; anything else is a Tier-1 configuration error naming the letter, rather
/// than a silent misinterpretation. Text between single quotes is a literal, and `''` is a
/// literal apostrophe, exactly as in Java.
///
/// ```
/// use cdm_codec::JavaDateFormat;
///
/// let format = JavaDateFormat::parse("yyyyMMddHHmmss")?;
/// let value = format.parse_datetime("20220412215715")?;
/// assert_eq!(format.format_datetime(&value), "20220412215715");
/// # Ok::<(), cdm_core::CdmError>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JavaDateFormat {
    pattern: String,
    fields: Vec<Field>,
}

impl JavaDateFormat {
    /// Translates a Java pattern.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Config`] — a Tier-1 error (`CDC-021`, `CDC-022`) — when the pattern is
    /// empty, when a quoted literal is unterminated, or when it uses a pattern letter cdm-rs does
    /// not support. The message names the letter.
    pub fn parse(pattern: &str) -> Result<Self, CdmError> {
        if pattern.is_empty() {
            return Err(config_error(
                "transform.codecs.timestamp_format is required and cannot be empty",
            ));
        }

        let mut fields = Vec::new();
        let chars: Vec<char> = pattern.chars().collect();
        let mut index = 0;
        while let Some(&c) = chars.get(index) {
            if c == '\'' {
                index += 1;
                let mut literal = String::new();
                loop {
                    match chars.get(index) {
                        None => {
                            return Err(config_error(format!(
                                "unterminated quoted literal in date format `{pattern}`"
                            )))
                        }
                        Some('\'') if chars.get(index + 1) == Some(&'\'') => {
                            literal.push('\'');
                            index += 2;
                        }
                        Some('\'') => {
                            index += 1;
                            break;
                        }
                        Some(&other) => {
                            literal.push(other);
                            index += 1;
                        }
                    }
                }
                // Java reads `''` outside a quoted section as a literal apostrophe too.
                if literal.is_empty() {
                    literal.push('\'');
                }
                fields.push(Field::Literal(literal));
                continue;
            }

            if !c.is_ascii_alphabetic() {
                let mut literal = String::new();
                while let Some(&other) = chars.get(index) {
                    if other.is_ascii_alphabetic() || other == '\'' {
                        break;
                    }
                    literal.push(other);
                    index += 1;
                }
                fields.push(Field::Literal(literal));
                continue;
            }

            let mut width = 0;
            while chars.get(index) == Some(&c) {
                width += 1;
                index += 1;
            }
            fields.push(letter_field(c, width, pattern)?);
        }

        Ok(Self {
            pattern: pattern.to_owned(),
            fields,
        })
    }

    /// The pattern this format was built from, as written.
    pub fn pattern(&self) -> &str {
        &self.pattern
    }

    /// Renders a local date-time.
    pub fn format_datetime(&self, value: &NaiveDateTime) -> String {
        use chrono::Datelike as _;
        let mut out = String::new();
        for field in &self.fields {
            match field {
                Field::Literal(text) => out.push_str(text),
                Field::Year(w) => {
                    let (width, year) = (*w, value.year());
                    if width == 2 {
                        let _ = write!(out, "{:02}", year.rem_euclid(100));
                    } else {
                        let _ = write!(out, "{year:0width$}");
                    }
                }
                Field::Month(w) => {
                    let width = *w;
                    let _ = write!(out, "{:0width$}", value.month());
                }
                Field::Day(w) => {
                    let width = *w;
                    let _ = write!(out, "{:0width$}", value.day());
                }
                Field::Hour24(w) => {
                    let width = *w;
                    let _ = write!(out, "{:0width$}", value.hour());
                }
                Field::Hour12(w) => {
                    let width = *w;
                    let hour = value.hour() % 12;
                    let hour = if hour == 0 { 12 } else { hour };
                    let _ = write!(out, "{hour:0width$}");
                }
                Field::Minute(w) => {
                    let width = *w;
                    let _ = write!(out, "{:0width$}", value.minute());
                }
                Field::Second(w) => {
                    let width = *w;
                    let _ = write!(out, "{:0width$}", value.second());
                }
                Field::Fraction(w) => {
                    let width = *w;
                    let nanos = value.nanosecond();
                    let digits = format!("{nanos:09}");
                    let truncated = digits.get(..width.min(9)).unwrap_or(&digits);
                    out.push_str(truncated);
                    for _ in 9..width {
                        out.push('0');
                    }
                }
                Field::AmPm => out.push_str(if value.hour() < 12 { "AM" } else { "PM" }),
            }
        }
        out
    }

    /// Parses a local date-time.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::TypeConversion`] when the text does not match the pattern — the
    /// record-level equivalent of Java's `DateTimeParseException`.
    pub fn parse_datetime(&self, text: &str) -> Result<NaiveDateTime, CdmError> {
        let chars: Vec<char> = text.chars().collect();
        let mut index = 0;
        let mut parts = Parts::default();

        for field in &self.fields {
            match field {
                Field::Literal(literal) => {
                    for expected in literal.chars() {
                        if chars.get(index) != Some(&expected) {
                            return Err(self.parse_error(text));
                        }
                        index += 1;
                    }
                }
                Field::AmPm => {
                    let marker: String = chars
                        .get(index..index + 2)
                        .ok_or_else(|| self.parse_error(text))?
                        .iter()
                        .collect();
                    match marker.to_ascii_uppercase().as_str() {
                        "AM" => parts.pm = Some(false),
                        "PM" => parts.pm = Some(true),
                        _ => return Err(self.parse_error(text)),
                    }
                    index += 2;
                }
                _ => {
                    let width = field_width(field);
                    let digits = read_digits(&chars, &mut index, width)
                        .ok_or_else(|| self.parse_error(text))?;
                    let value: i64 = digits.parse().map_err(|_| self.parse_error(text))?;
                    match field {
                        Field::Year(w) => {
                            parts.year = Some(if *w == 2 { 2000 + value } else { value });
                        }
                        Field::Month(_) => parts.month = Some(value),
                        Field::Day(_) => parts.day = Some(value),
                        Field::Hour24(_) => parts.hour = Some(value),
                        Field::Hour12(_) => parts.hour12 = Some(value),
                        Field::Minute(_) => parts.minute = Some(value),
                        Field::Second(_) => parts.second = Some(value),
                        Field::Fraction(w) => {
                            parts.nanos = Some(scale_fraction(value, *w));
                        }
                        _ => {}
                    }
                }
            }
        }

        if index != chars.len() {
            return Err(self.parse_error(text));
        }
        parts.build().ok_or_else(|| self.parse_error(text))
    }

    fn parse_error(&self, text: &str) -> CdmError {
        CdmError::new(
            ErrorKind::TypeConversion,
            format!("`{text}` does not match date format `{}`", self.pattern),
        )
    }
}

const fn field_width(field: &Field) -> usize {
    match field {
        Field::Year(w)
        | Field::Month(w)
        | Field::Day(w)
        | Field::Hour24(w)
        | Field::Hour12(w)
        | Field::Minute(w)
        | Field::Second(w)
        | Field::Fraction(w) => *w,
        Field::Literal(_) | Field::AmPm => 0,
    }
}

/// Reads exactly `width` digits when the pattern pinned a width, or up to two otherwise.
fn read_digits(chars: &[char], index: &mut usize, width: usize) -> Option<String> {
    let wanted = if width <= 1 { 2 } else { width };
    let mut digits = String::new();
    while digits.len() < wanted {
        match chars.get(*index) {
            Some(c) if c.is_ascii_digit() => {
                digits.push(*c);
                *index += 1;
            }
            _ => break,
        }
    }
    if digits.is_empty() || (width > 1 && digits.len() != width) {
        return None;
    }
    Some(digits)
}

/// Scales `value`, which had `width` digits, to nanoseconds.
fn scale_fraction(value: i64, width: usize) -> i64 {
    let mut nanos = value;
    for _ in width..9 {
        nanos = nanos.saturating_mul(10);
    }
    for _ in 9..width {
        nanos /= 10;
    }
    nanos
}

#[derive(Debug, Default)]
struct Parts {
    year: Option<i64>,
    month: Option<i64>,
    day: Option<i64>,
    hour: Option<i64>,
    hour12: Option<i64>,
    minute: Option<i64>,
    second: Option<i64>,
    nanos: Option<i64>,
    pm: Option<bool>,
}

impl Parts {
    fn build(self) -> Option<NaiveDateTime> {
        let year = i32::try_from(self.year.unwrap_or(1970)).ok()?;
        let month = u32::try_from(self.month.unwrap_or(1)).ok()?;
        let day = u32::try_from(self.day.unwrap_or(1)).ok()?;
        let hour = match (self.hour, self.hour12) {
            (Some(hour), _) => u32::try_from(hour).ok()?,
            (None, Some(clock)) => {
                let clock = u32::try_from(clock).ok()? % 12;
                if self.pm == Some(true) {
                    clock + 12
                } else {
                    clock
                }
            }
            (None, None) => 0,
        };
        let minute = u32::try_from(self.minute.unwrap_or(0)).ok()?;
        let second = u32::try_from(self.second.unwrap_or(0)).ok()?;
        let nanos = u32::try_from(self.nanos.unwrap_or(0)).ok()?;
        NaiveDate::from_ymd_opt(year, month, day)?.and_hms_nano_opt(hour, minute, second, nanos)
    }
}

fn letter_field(letter: char, width: usize, pattern: &str) -> Result<Field, CdmError> {
    Ok(match letter {
        'y' | 'u' => Field::Year(width),
        'M' if width <= 2 => Field::Month(width),
        'd' => Field::Day(width),
        'H' => Field::Hour24(width),
        'h' => Field::Hour12(width),
        'm' => Field::Minute(width),
        's' => Field::Second(width),
        'S' => Field::Fraction(width),
        'a' => Field::AmPm,
        'M' => {
            return Err(config_error(format!(
                "date format `{pattern}` uses the textual month pattern `{}`, which cdm-rs does \
                 not support; use `MM` (numeric) instead",
                "M".repeat(width)
            )))
        }
        other => {
            return Err(config_error(format!(
                "date format `{pattern}` uses the unsupported pattern letter `{other}`; \
                 cdm-rs supports `y M d H h m s S a`"
            )))
        }
    })
}

fn config_error(message: impl Into<String>) -> CdmError {
    CdmError::new(ErrorKind::Config, message)
        .with_context(|c| c.with_config_key("transform.codecs.timestamp_format"))
}

// Tests may panic freely: a failed assertion *is* the reporting mechanism, and the no-panic rule
// (ERR-004) exists to protect production paths, not test bodies.
#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::float_cmp
)]
mod tests {
    use super::*;

    #[test]
    fn cdc_020_double_string_never_uses_scientific_notation() {
        // Double.toString would render these as 2.14748364707E10 and 1.0E20; the whole reason
        // DOUBLE_StringCodec exists is that a `text` column must not receive that.
        assert_eq!(format_double_java(21_474_836_470.7), "21474836470.7");
        assert_eq!(format_double_java(1e20), "100000000000000000000");
        assert_eq!(format_double_java(1e-7), "0.0000001");
        assert!(!format_double_java(1e300).contains('E'));
    }

    #[test]
    fn cdc_020_double_string_uses_at_most_nine_fraction_digits_rounded_towards_negative_infinity() {
        // RoundingMode.FLOOR, not HALF_EVEN: the tenth digit is discarded downwards.
        assert_eq!(format_double_java(1.234_567_891_5), "1.234567891");
        assert_eq!(format_double_java(-1.234_567_891_5), "-1.234567892");
        assert_eq!(format_double_java(0.5), "0.5");
    }

    #[test]
    fn cdc_020_double_string_drops_trailing_zeros_and_keeps_one_integer_digit() {
        assert_eq!(format_double_java(1.0), "1");
        assert_eq!(format_double_java(0.0), "0");
        assert_eq!(format_double_java(-0.0), "-0");
        assert_eq!(format_double_java(0.25), "0.25");
        assert_eq!(format_double_java(1_000_000.0), "1000000");
    }

    #[test]
    fn cdc_020_double_string_uses_the_locale_us_symbols_for_non_finite_values() {
        assert_eq!(format_double_java(f64::NAN), "NaN");
        assert_eq!(format_double_java(f64::INFINITY), "∞");
        assert_eq!(format_double_java(f64::NEG_INFINITY), "-∞");
    }

    #[test]
    fn cdc_020_double_parsing_rejects_non_numbers() {
        assert_eq!(
            parse_double_java("21474836470.7").unwrap(),
            21_474_836_470.7
        );
        assert_eq!(parse_double_java("2.1e3").unwrap(), 2100.0);
        let error = parse_double_java("not a number").unwrap_err();
        assert_eq!(error.kind(), ErrorKind::TypeConversion);
    }

    #[test]
    fn cdc_022_java_patterns_are_accepted_verbatim() {
        let format = JavaDateFormat::parse("yyyyMMddHHmmss").unwrap();
        assert_eq!(format.pattern(), "yyyyMMddHHmmss");
        let value = format.parse_datetime("20220412215715").unwrap();
        assert_eq!(value.to_string(), "2022-04-12 21:57:15");
        assert_eq!(format.format_datetime(&value), "20220412215715");
    }

    #[test]
    fn cdc_022_two_digit_years_use_javas_base_2000_reduced_value() {
        // Java's `yy` is a reduced-value field based at 2000, so 22 is 2022 and 69 is 2069 —
        // unlike C's `%y`, which would make 69 mean 1969.
        let format = JavaDateFormat::parse("yyMMddHHmmss").unwrap();
        assert_eq!(
            format.parse_datetime("220412215715").unwrap().to_string(),
            "2022-04-12 21:57:15"
        );
        assert_eq!(
            format.parse_datetime("690412215715").unwrap().to_string(),
            "2069-04-12 21:57:15"
        );
        let value = format.parse_datetime("220412215715").unwrap();
        assert_eq!(format.format_datetime(&value), "220412215715");
    }

    #[test]
    fn cdc_022_literals_separators_fractions_and_am_pm_round_trip() {
        let format = JavaDateFormat::parse("yyyy-MM-dd'T'hh:mm:ss.SSS a").unwrap();
        let text = "2022-04-12T09:57:15.123 PM";
        let value = format.parse_datetime(text).unwrap();
        assert_eq!(value.to_string(), "2022-04-12 21:57:15.123");
        assert_eq!(format.format_datetime(&value), text);
    }

    #[test]
    fn cdc_022_an_unsupported_pattern_letter_is_a_tier_1_error_naming_the_letter() {
        let error = JavaDateFormat::parse("yyyy-MM-dd'T'HH:mm:ssXXX").unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Config);
        assert!(error.to_string().contains("`X`"), "{error}");

        let error = JavaDateFormat::parse("EEE, dd MMM yyyy").unwrap_err();
        assert!(error.to_string().contains("`E`"), "{error}");

        let error = JavaDateFormat::parse("dd MMM yyyy").unwrap_err();
        assert!(error.to_string().contains("MMM"), "{error}");
    }

    #[test]
    fn cdc_021_an_empty_pattern_is_a_tier_1_error() {
        let error = JavaDateFormat::parse("").unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Config);
        assert!(error.to_string().contains("cannot be empty"), "{error}");
    }

    #[test]
    fn cdc_022_an_unterminated_quoted_literal_is_a_tier_1_error() {
        let error = JavaDateFormat::parse("yyyy'T").unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Config);
        assert!(error.to_string().contains("unterminated"), "{error}");
    }

    #[test]
    fn cdc_022_text_that_does_not_match_the_pattern_is_a_record_level_error() {
        let format = JavaDateFormat::parse("yyyyMMddHHmmss").unwrap();
        for bad in ["not a valid format", "2022041221571", "202204122157155"] {
            let error = format.parse_datetime(bad).unwrap_err();
            assert_eq!(error.kind(), ErrorKind::TypeConversion, "{bad}");
        }
        // A field that parses but is out of range fails too, rather than wrapping around.
        assert!(format.parse_datetime("20221312215715").is_err());
    }

    #[test]
    fn cdc_022_an_escaped_apostrophe_is_a_literal() {
        let format = JavaDateFormat::parse("yyyy''MM").unwrap();
        let value = format.parse_datetime("2022'04").unwrap();
        assert_eq!(format.format_datetime(&value), "2022'04");
    }
}
