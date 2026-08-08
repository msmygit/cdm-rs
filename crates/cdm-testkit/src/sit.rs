//! The declarative harness for the ported Java SIT cases (`TST-003`).
//!
//! Java's SIT is a directory per case and four shell scripts. Each case directory holds a
//! `setup.cql`, one or more `.properties` files, a `cdm.txt` naming the scenarios, an `execute.sh`
//! sequencing them, a `cdm.<scenario>.assert` holding the expected counter block, and an
//! `expected.cql`/`expected.out` pair describing the final state of the target. `test.sh` copies
//! the directory into two containers, runs `setup.cql` through `cqlsh`, runs `execute.sh`, then
//! runs `expected.cql` and diffs its output against `expected.out`.
//!
//! This module is the same idea with the shell removed. A case is still a directory; `execute.sh`
//! and `cdm.txt` collapse into one [`case.txt`](SitCase::load) step list, because they always said
//! the same thing twice and a case whose two halves disagree is a case nobody can read. Everything
//! here is pure: it parses a case, renders a properties file against a contact point, extracts a
//! counter block from a run's stdout and compares two `cqlsh` result sets. Actually *running* a
//! case needs a node and the `cdm` binary, and lives in `tests/sit_it.rs`.
//!
//! # Why the harness drives the binary rather than the library
//!
//! Java's SIT drives `spark-submit`, not `CopyJobSession`. A parity suite that reached past the
//! command line would prove the engine agrees with Java while saying nothing about whether the
//! thing an operator types does, and the gap between those two is exactly where this port found
//! its unwired features. So `sit_it.rs` runs `cdm migrate`/`cdm validate` as a subprocess and
//! reads their stdout, which also keeps this crate free of the `cdm-engine` and `cdm-cql`
//! dependencies `ARCHITECTURE.md` §3 forbids it.
//!
//! # Why the expectations were regenerated rather than copied
//!
//! The `.assert` and `expected.out` files in the Java tree encode Java's behaviour, including the
//! parts of it cdm-rs deliberately does not reproduce (`docs/MIGRATION_FROM_JAVA.md`). Copying
//! them across would have made two known defects — the unreachable migrate flush threshold
//! (`MIG-004`, divergence 15) and the permanently-zero validate error count (`ENG-008`,
//! divergence 16) — into cdm-rs's *expected* behaviour. Every expectation in `tests/sit/` was
//! therefore derived from `docs/SPEC.md` and then confirmed against a real run; where one differs
//! from Java's, the case file says which divergence explains it.

use std::path::{Path, PathBuf};

use cdm_core::{CdmError, ErrorKind};

/// The name of the step list in each case directory.
///
/// It replaces Java's `cdm.txt` (scenario → class → properties) and `execute.sh` (the order they
/// run in, and where `breakData.cql` is applied).
pub const CASE_FILE: &str = "case.txt";

/// The `cdm` subcommand a [`SitStep::Job`] runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SitJob {
    /// `cdm migrate` — Java's `com.datastax.cdm.job.Migrate`.
    Migrate,
    /// `cdm validate` — Java's `com.datastax.cdm.job.DiffData`.
    Validate,
    /// `cdm guardrail` — Java's `com.datastax.cdm.job.GuardrailCheck`.
    Guardrail,
}

impl SitJob {
    /// The subcommand as it is spelled on the command line.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Migrate => "migrate",
            Self::Validate => "validate",
            Self::Guardrail => "guardrail",
        }
    }

    /// Parses a subcommand name.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Config`] if `name` is not one of the three jobs.
    pub fn parse(name: &str) -> Result<Self, CdmError> {
        match name {
            "migrate" => Ok(Self::Migrate),
            "validate" => Ok(Self::Validate),
            "guardrail" => Ok(Self::Guardrail),
            other => Err(CdmError::new(
                ErrorKind::Config,
                format!("`{other}` is not a cdm job; expected migrate, validate or guardrail"),
            )),
        }
    }
}

/// One line of a case's step list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SitStep {
    /// `cql <file>` — run a CQL script against the cluster.
    ///
    /// Covers both of Java's uses: `setup.cql` before anything runs, and `breakData.cql` between
    /// two scenarios.
    Cql {
        /// The script, relative to the case directory.
        file: String,
    },
    /// `job <migrate|validate|guardrail> <properties> <assert>` — run a job and check its block.
    Job {
        /// Which `cdm` subcommand to run.
        job: SitJob,
        /// The properties file, relative to the case directory.
        properties: String,
        /// The expected counter block, relative to the case directory.
        expect: String,
    },
    /// `check <query.cql> <expected.out>` — assert the final state of the target.
    Check {
        /// The query script, relative to the case directory.
        query: String,
        /// The expected `cqlsh` output, relative to the case directory.
        expected: String,
    },
}

impl SitStep {
    /// Every file this step reads, relative to the case directory.
    #[must_use]
    pub fn files(&self) -> Vec<&str> {
        match self {
            Self::Cql { file } => vec![file.as_str()],
            Self::Job {
                properties, expect, ..
            } => vec![properties.as_str(), expect.as_str()],
            Self::Check { query, expected } => vec![query.as_str(), expected.as_str()],
        }
    }
}

/// A ported Java SIT case: a directory, a step list, and the requirements it covers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SitCase {
    phase: String,
    name: String,
    dir: PathBuf,
    steps: Vec<SitStep>,
    blocked: Option<String>,
}

impl SitCase {
    /// Loads the case rooted at `dir`, whose parent directory names the phase.
    ///
    /// The step list is `case.txt`: one step per line, `#` starts a comment, blank lines are
    /// ignored, and fields are separated by whitespace.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Config`] if the directory is not shaped like a case, if `case.txt` cannot be
    /// read or parsed, or if a step names a file that is not there. The last is checked here
    /// rather than at run time so that a typo fails in the manifest test that runs on every pull
    /// request, not only in the Docker-backed suite that does not.
    pub fn load(dir: &Path) -> Result<Self, CdmError> {
        let name = component(dir, "case")?;
        let phase = dir
            .parent()
            .ok_or_else(|| shape(dir, "has no parent directory to name its phase"))
            .and_then(|parent| component(parent, "phase"))?;

        let case_file = dir.join(CASE_FILE);
        let text = std::fs::read_to_string(&case_file).map_err(|e| {
            CdmError::new(
                ErrorKind::Config,
                format!("cannot read {}: {e}", case_file.display()),
            )
        })?;

        let steps = parse_steps(&text, &case_file)?;
        if steps.is_empty() {
            return Err(shape(&case_file, "declares no steps"));
        }
        let blocked = parse_blocked(&text);

        for step in &steps {
            for file in step.files() {
                let path = dir.join(file);
                if !path.is_file() {
                    return Err(shape(
                        &case_file,
                        &format!("names `{file}`, which does not exist in the case directory"),
                    ));
                }
            }
        }

        Ok(Self {
            phase,
            name,
            dir: dir.to_path_buf(),
            steps,
            blocked,
        })
    }

    /// Why this case cannot run yet, if it cannot.
    ///
    /// A `blocked <reason>` line in `case.txt`. It exists because `#[ignore]` cannot carry the
    /// distinction the suite needs: `cargo test -- --ignored` runs *only* ignored tests, and every
    /// case in this suite is ignored because every case needs a container. A second signal is
    /// therefore required to say "and this one additionally cannot pass", and putting it in the
    /// fixture rather than in the test keeps the reason next to the case it is about — and makes
    /// the count of blocked cases something a script can read.
    #[must_use]
    pub fn blocked(&self) -> Option<&str> {
        self.blocked.as_deref()
    }

    /// The phase directory: `smoke`, `features` or `regression`, as in the Java tree.
    #[must_use]
    pub fn phase(&self) -> &str {
        &self.phase
    }

    /// The case directory's name, e.g. `01_basic_kvp`.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// `phase/name`, which is how the Java suite refers to a case.
    #[must_use]
    pub fn id(&self) -> String {
        format!("{}/{}", self.phase, self.name)
    }

    /// The case directory.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The steps, in the order they run.
    #[must_use]
    pub fn steps(&self) -> &[SitStep] {
        &self.steps
    }

    /// Reads one of the case's files.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Config`] if the file cannot be read.
    pub fn read(&self, file: &str) -> Result<String, CdmError> {
        let path = self.dir.join(file);
        std::fs::read_to_string(&path).map_err(|e| {
            CdmError::new(
                ErrorKind::Config,
                format!("cannot read {}: {e}", path.display()),
            )
        })
    }
}

/// The directory holding the ported cases, `tests/sit/` at the repository root.
///
/// Derived from this crate's manifest directory rather than the current working directory: a test
/// binary's working directory is its package root, and the fixtures deliberately sit outside any
/// package so that no crate owns the parity suite.
#[must_use]
pub fn sit_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("tests")
        .join("sit")
}

/// Every ported case, ordered by phase and then by case name.
///
/// # Errors
///
/// [`ErrorKind::Config`] if `tests/sit/` cannot be walked or a case cannot be loaded.
pub fn cases() -> Result<Vec<SitCase>, CdmError> {
    let root = sit_root();
    let mut phases = read_dir(&root)?;
    phases.sort();

    let mut cases = Vec::new();
    for phase in phases {
        if !phase.is_dir() {
            continue;
        }
        let mut dirs = read_dir(&phase)?;
        dirs.sort();
        for dir in dirs {
            if dir.join(CASE_FILE).is_file() {
                cases.push(SitCase::load(&dir)?);
            }
        }
    }
    Ok(cases)
}

/// Loads one case by `phase` and `name`.
///
/// # Errors
///
/// [`ErrorKind::Config`] if the case does not exist or cannot be loaded.
pub fn case(phase: &str, name: &str) -> Result<SitCase, CdmError> {
    SitCase::load(&sit_root().join(phase).join(name))
}

/// Substitutes the contact point into a properties file.
///
/// A ported `.properties` file writes `{{host}}` and `{{port}}` where Java wrote `cdm-sit-cass`,
/// because the fixture's port is chosen at run time. Substituting into a copy — rather than
/// overriding on the command line — keeps the file the single statement of what the run is
/// configured with, which is the property a parity suite most needs to be able to read off.
#[must_use]
pub fn render_properties(template: &str, host: &str, port: u16) -> String {
    template
        .replace("{{host}}", host)
        .replace("{{port}}", &port.to_string())
}

/// Rewrites a CQL script into the one line `cqlsh -e` will accept.
///
/// `cqlsh -e` takes its whole argument as a single string and parses it with the same lexer it
/// uses interactively, which trips over a blank line between two statements — it sees an empty
/// statement and answers `no viable alternative at input ';'`. Comments have the same effect. The
/// ported fixtures are written to be *read*, with blank lines between records and a comment
/// explaining every deliberately odd value, so the harness strips both here rather than the
/// fixtures giving them up.
///
/// String literals are respected: `'A---'` is a value in `smoke/03_ttl_writetime` and must not be
/// mistaken for the start of a comment.
#[must_use]
pub fn flatten_cql(script: &str) -> String {
    let mut out = String::with_capacity(script.len());
    for line in script.lines() {
        let code = strip_comment(line).trim_end();
        if code.trim().is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(code);
    }
    out
}

/// The largest script, in bytes, that is handed to `cqlsh -e` in one go.
///
/// `execve` caps a single argument at `MAX_ARG_STRLEN`, 128 KiB on Linux, and
/// `regression/03_performance`'s four thousand inserts are three times that: the container answers
/// `argument list too long` before `cqlsh` starts. Chunking is the fixture's problem, not the
/// fixture author's.
const MAX_SCRIPT_BYTES: usize = 60 * 1024;

/// Splits a flattened script into chunks small enough for one `cqlsh -e`.
///
/// Statements are never split: the boundary is a `;` outside a string literal, so a multi-line
/// `INSERT` of a UDT stays whole.
#[must_use]
pub fn chunk_cql(script: &str) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();
    for statement in split_statements(script) {
        if !current.is_empty() && current.len() + statement.len() > MAX_SCRIPT_BYTES {
            chunks.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push('\n');
        }
        current.push_str(statement.trim());
    }
    if !current.trim().is_empty() {
        chunks.push(current);
    }
    chunks
}

/// Splits on `;` outside a single-quoted literal, keeping the `;`.
fn split_statements(script: &str) -> Vec<&str> {
    let bytes = script.as_bytes();
    let mut out = Vec::new();
    let mut start = 0;
    let mut in_string = false;
    for (index, byte) in bytes.iter().enumerate() {
        match byte {
            b'\'' => in_string = !in_string,
            b';' if !in_string => {
                #[allow(clippy::indexing_slicing)]
                out.push(&script[start..=index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    #[allow(clippy::indexing_slicing)]
    if start < script.len() && !script[start..].trim().is_empty() {
        out.push(&script[start..]);
    }
    out
}

/// Drops a `--` comment from `line`, ignoring one inside a single-quoted literal.
fn strip_comment(line: &str) -> &str {
    let bytes = line.as_bytes();
    let mut in_string = false;
    let mut i = 0;
    while i < bytes.len() {
        // SAFETY-INVARIANT: `i < bytes.len()` is the loop condition, so both reads are in bounds;
        // the second is guarded by its own bound check.
        #[allow(clippy::indexing_slicing)]
        let byte = bytes[i];
        if byte == b'\'' {
            in_string = !in_string;
        } else if !in_string && byte == b'-' && bytes.get(i + 1) == Some(&b'-') {
            #[allow(clippy::indexing_slicing)]
            return &line[..i];
        }
        i += 1;
    }
    line
}

/// The final counter block a run printed, with the `Final ` prefix stripped (`MET-006`).
///
/// This is Java's `egrep 'JobCounter.* Final ' | sed 's/^.*Final //'` from `cdm-assert.sh`, which
/// is what makes a ported `.assert` file directly comparable with the Java original.
#[must_use]
pub fn final_counter_block(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .filter_map(|line| line.trim().strip_prefix("Final "))
        .map(str::to_owned)
        .collect()
}

/// Compares a run's counter block against an expectation file.
///
/// Line for line and in order, which is stricter than Java: `cdm-assert.sh` diffs the two files,
/// but Java's own fixtures disagree with each other about the order — `features/08`'s block puts
/// `Write` before `Skipped` where every other migrate case puts it after — so Java's ordering
/// cannot be the contract. cdm-rs emits one order (`MET-006`), every ported expectation is
/// written in it, and asserting that is what makes the block a contract rather than a bag.
///
/// # Errors
///
/// A description of the first difference, with both blocks, if they are not identical.
pub fn compare_counter_block(expected: &str, actual_stdout: &str) -> Result<(), String> {
    let want: Vec<&str> = expected
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect();
    let got = final_counter_block(actual_stdout);

    if got.is_empty() {
        return Err(format!(
            "no `Final …` lines in the run's output; was the counter block (MET-006) printed at \
             all?\n--- stdout ---\n{actual_stdout}"
        ));
    }
    if want.len() == got.len() && want.iter().zip(&got).all(|(w, g)| *w == g.as_str()) {
        return Ok(());
    }
    Err(format!(
        "counter block differs (TST-003 requires it to be identical)\n\
         --- expected ---\n{}\n--- actual ---\n{}",
        want.join("\n"),
        got.join("\n")
    ))
}

/// One result set as `cqlsh` renders it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CqlshTable {
    /// The column names, in the order `cqlsh` printed them.
    pub header: Vec<String>,
    /// The data rows, each whitespace-normalised, sorted.
    pub rows: Vec<String>,
    /// The `(N rows)` trailer, when `cqlsh` printed one.
    pub row_count: Option<usize>,
}

/// Parses every result set in a `cqlsh` transcript.
///
/// `cqlsh` renders a `SELECT` as a header line, a rule of dashes, the rows, a blank line and
/// `(N rows)`. A script with several `SELECT`s produces several of those in sequence.
///
/// The rows are **sorted**, and that is deliberate. Java's `expected.out` files record the order
/// `cqlsh` happened to return, which for a partition-key scan is token order — a fact about the
/// murmur3 hash of the fixture's keys, not about the migration. Java's own `smoke/02` fixture has
/// `key1, key3, key2` for exactly that reason. Asserting it would make the suite fail on a
/// different partitioner and prove nothing either way, so the harness asserts the *set* of rows
/// and the count, and leaves ordering to the tests that are actually about ordering (`FEA-020`,
/// `VAL-006`).
#[must_use]
pub fn parse_cqlsh(text: &str) -> Vec<CqlshTable> {
    let mut tables = Vec::new();
    let mut header: Option<Vec<String>> = None;
    let mut rows: Vec<String> = Vec::new();

    let flush = |header: &mut Option<Vec<String>>,
                 rows: &mut Vec<String>,
                 count: Option<usize>,
                 tables: &mut Vec<CqlshTable>| {
        if let Some(header) = header.take() {
            rows.sort();
            tables.push(CqlshTable {
                header,
                rows: std::mem::take(rows),
                row_count: count,
            });
        }
    };

    // The rule of dashes is the only reliable marker: a single-column result set has no `|` at
    // all, so "the line before the rule is the header" is what tells a header from a row, and a
    // banner or a warning outside any table from either.
    let mut previous = "";
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            previous = "";
            continue;
        }
        if let Some(count) = row_count_line(trimmed) {
            flush(&mut header, &mut rows, Some(count), &mut tables);
            previous = "";
            continue;
        }
        if is_rule(trimmed) && !previous.is_empty() {
            flush(&mut header, &mut rows, None, &mut tables);
            header = Some(cells(previous));
            previous = "";
            continue;
        }
        if header.is_some() {
            rows.push(cells(trimmed).join(" | "));
        }
        previous = trimmed;
    }
    flush(&mut header, &mut rows, None, &mut tables);
    tables
}

/// Splits a rendered row into trimmed cells.
fn cells(line: &str) -> Vec<String> {
    line.split('|').map(|c| c.trim().to_owned()).collect()
}

/// Compares an expected `cqlsh` transcript against an actual one.
///
/// # Errors
///
/// A description of the first difference: a differing number of result sets, differing columns,
/// a differing row count, or differing rows.
pub fn compare_cqlsh(expected: &str, actual: &str) -> Result<(), String> {
    let want = parse_cqlsh(expected);
    let got = parse_cqlsh(actual);

    if want.len() != got.len() {
        return Err(format!(
            "expected {} result set(s), got {}\n--- expected ---\n{expected}\n--- actual ---\n{actual}",
            want.len(),
            got.len()
        ));
    }
    for (index, (want, got)) in want.iter().zip(&got).enumerate() {
        if want.header != got.header {
            return Err(format!(
                "result set {index}: columns differ\n  expected: {:?}\n  actual:   {:?}",
                want.header, got.header
            ));
        }
        if want.row_count.is_some() && want.row_count != got.row_count {
            return Err(format!(
                "result set {index}: expected {:?} rows, got {:?}",
                want.row_count, got.row_count
            ));
        }
        if want.rows != got.rows {
            return Err(format!(
                "result set {index}: rows differ (both sorted; order is not asserted)\n\
                 --- expected ---\n{}\n--- actual ---\n{}",
                want.rows.join("\n"),
                got.rows.join("\n")
            ));
        }
    }
    Ok(())
}

/// `(N rows)` or `(N row)`, as `cqlsh` prints it.
fn row_count_line(line: &str) -> Option<usize> {
    let inner = line.strip_prefix('(')?.strip_suffix(')')?;
    let (count, unit) = inner.split_once(' ')?;
    (unit == "rows" || unit == "row").then(|| count.parse().ok())?
}

/// The `----+----` rule `cqlsh` draws under a header.
fn is_rule(line: &str) -> bool {
    !line.is_empty() && line.chars().all(|c| c == '-' || c == '+' || c == ' ')
}

/// The `blocked <reason>` line of a `case.txt`, if there is one.
fn parse_blocked(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find_map(|line| line.strip_prefix("blocked "))
        .map(|reason| reason.trim().to_owned())
}

/// Parses `case.txt`.
fn parse_steps(text: &str, path: &Path) -> Result<Vec<SitStep>, CdmError> {
    let mut steps = Vec::new();
    for (number, raw) in text.lines().enumerate() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split_whitespace().collect();
        let at = |n: usize| -> Result<String, CdmError> {
            fields.get(n).map(|s| (*s).to_owned()).ok_or_else(|| {
                CdmError::new(
                    ErrorKind::Config,
                    format!(
                        "{}:{}: `{line}` is missing field {n}",
                        path.display(),
                        number + 1
                    ),
                )
            })
        };
        let step = match fields.first().copied() {
            // A header directive, not a step; `parse_blocked` reads it.
            Some("blocked") => continue,
            Some("cql") => SitStep::Cql { file: at(1)? },
            Some("job") => SitStep::Job {
                job: SitJob::parse(&at(1)?)?,
                properties: at(2)?,
                expect: at(3)?,
            },
            Some("check") => SitStep::Check {
                query: at(1)?,
                expected: at(2)?,
            },
            other => {
                return Err(CdmError::new(
                    ErrorKind::Config,
                    format!(
                        "{}:{}: `{}` is not a step kind; expected cql, job or check",
                        path.display(),
                        number + 1,
                        other.unwrap_or("")
                    ),
                ))
            }
        };
        steps.push(step);
    }
    Ok(steps)
}

fn component(dir: &Path, what: &str) -> Result<String, CdmError> {
    dir.file_name()
        .and_then(std::ffi::OsStr::to_str)
        .map(str::to_owned)
        .ok_or_else(|| shape(dir, &format!("has no {what} name")))
}

fn shape(path: &Path, problem: &str) -> CdmError {
    CdmError::new(ErrorKind::Config, format!("{} {problem}", path.display()))
}

fn read_dir(dir: &Path) -> Result<Vec<PathBuf>, CdmError> {
    let entries = std::fs::read_dir(dir).map_err(|e| {
        CdmError::new(
            ErrorKind::Config,
            format!("cannot read {}: {e}", dir.display()),
        )
    })?;
    entries
        .map(|entry| {
            entry.map(|e| e.path()).map_err(|e| {
                CdmError::new(
                    ErrorKind::Config,
                    format!("cannot read an entry of {}: {e}", dir.display()),
                )
            })
        })
        .collect()
}

// Tests may panic freely: a failed assertion is the reporting mechanism (see AGENTS.md).
#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
mod tests {
    use super::*;

    /// The nineteen Java SIT cases, as `docs/SPEC.md` `TST-003` enumerates them.
    const EXPECTED: [(&str, &str); 19] = [
        ("smoke", "00_test_harness"),
        ("smoke", "01_basic_kvp"),
        ("smoke", "02_autocorrect_kvp"),
        ("smoke", "03_ttl_writetime"),
        ("smoke", "04_counters"),
        ("smoke", "05_reserved_keyword"),
        ("smoke", "06_vector"),
        ("features", "01_constant_column"),
        ("features", "02_explode_map"),
        ("features", "03_codec"),
        ("features", "04_udt_mapper"),
        ("features", "05_guardrail"),
        ("features", "06_constant_column_remove"),
        ("features", "07_constant_column_replace"),
        ("features", "08_map_columns_origin_target"),
        ("regression", "01_explode_map_with_constants"),
        ("regression", "02_ColumnRenameWithConstantsAndExplode"),
        ("regression", "03_performance"),
        ("regression", "04_null_ts_in_pk"),
    ];

    #[test]
    fn tst_003_every_java_sit_case_is_ported_and_loads() {
        let cases = cases().expect("tests/sit must be walkable");
        let ids: Vec<String> = cases.iter().map(SitCase::id).collect();
        for (phase, name) in EXPECTED {
            let id = format!("{phase}/{name}");
            assert!(
                ids.contains(&id),
                "the Java SIT case {id} has no port in tests/sit; ids present: {ids:?}"
            );
        }
        assert_eq!(
            cases.len(),
            EXPECTED.len(),
            "tests/sit holds {} cases but TST-003 enumerates {}: {ids:?}",
            cases.len(),
            EXPECTED.len()
        );
    }

    #[test]
    fn tst_003_a_case_names_only_files_that_exist() {
        // `SitCase::load` enforces this, so loading every case is the assertion. The value is in
        // where it fails: on every pull request, rather than in the Docker-backed suite that a
        // typo would otherwise reach only after a container has started.
        for case in cases().expect("tests/sit must be walkable") {
            assert!(!case.steps().is_empty(), "{} declares no steps", case.id());
        }
    }

    #[test]
    fn tst_003_every_case_ends_by_checking_the_targets_final_state() {
        for case in cases().expect("tests/sit must be walkable") {
            let last = case.steps().last().expect("a case has steps");
            assert!(
                matches!(last, SitStep::Check { .. }),
                "{} does not end with a `check` step; a parity case that never looks at the \
                 target proves only that the run exited",
                case.id()
            );
        }
    }

    #[test]
    fn tst_003_a_step_list_parses_kinds_comments_and_blank_lines() {
        let text = "\
# a comment
cql setup.cql

job migrate migrate.properties migrate.assert   # trailing comment
job validate migrate.properties validate.assert
cql break.cql
check expected.cql expected.out
";
        let steps = parse_steps(text, Path::new("case.txt")).expect("parses");
        assert_eq!(
            steps,
            vec![
                SitStep::Cql {
                    file: "setup.cql".to_owned()
                },
                SitStep::Job {
                    job: SitJob::Migrate,
                    properties: "migrate.properties".to_owned(),
                    expect: "migrate.assert".to_owned(),
                },
                SitStep::Job {
                    job: SitJob::Validate,
                    properties: "migrate.properties".to_owned(),
                    expect: "validate.assert".to_owned(),
                },
                SitStep::Cql {
                    file: "break.cql".to_owned()
                },
                SitStep::Check {
                    query: "expected.cql".to_owned(),
                    expected: "expected.out".to_owned(),
                },
            ]
        );
    }

    #[test]
    fn tst_003_an_unknown_step_kind_is_rejected_rather_than_ignored() {
        let err = parse_steps("dsbulk load data.csv", Path::new("case.txt")).unwrap_err();
        assert!(format!("{err}").contains("not a step kind"), "{err}");
    }

    #[test]
    fn tst_003_a_properties_template_takes_the_fixtures_contact_point() {
        let rendered = render_properties(
            "spark.cdm.connect.origin.host {{host}}\nspark.cdm.connect.origin.port {{port}}\n",
            "127.0.0.1",
            9042,
        );
        assert_eq!(
            rendered,
            "spark.cdm.connect.origin.host 127.0.0.1\nspark.cdm.connect.origin.port 9042\n"
        );
    }

    #[test]
    fn tst_003_the_counter_block_is_extracted_exactly_as_cdm_assert_sh_extracts_it() {
        let stdout = "\
################################################################################################
RunId: 0
Final Read Record Count: 2
Final Write Record Count: 2
Final Skipped Record Count: 0
Final Error Record Count: 0
Final Partitions Passed: 1
Final Partitions Failed: 0
################################################################################################

migrate ENDED: 1 range(s) passed, 0 failed.
";
        assert_eq!(
            final_counter_block(stdout),
            vec![
                "Read Record Count: 2",
                "Write Record Count: 2",
                "Skipped Record Count: 0",
                "Error Record Count: 0",
                "Partitions Passed: 1",
                "Partitions Failed: 0",
            ]
        );
        assert!(compare_counter_block(
            "Read Record Count: 2\nWrite Record Count: 2\nSkipped Record Count: 0\n\
             Error Record Count: 0\nPartitions Passed: 1\nPartitions Failed: 0\n",
            stdout
        )
        .is_ok());
    }

    #[test]
    fn tst_003_a_differing_counter_value_is_reported_with_both_blocks() {
        let err = compare_counter_block("Read Record Count: 3\n", "Final Read Record Count: 2\n")
            .unwrap_err();
        assert!(err.contains("Read Record Count: 3"), "{err}");
        assert!(err.contains("Read Record Count: 2"), "{err}");
    }

    #[test]
    fn tst_003_a_run_that_printed_no_block_is_a_failure_not_an_empty_match() {
        // ENG-008's whole point is that a counter nobody printed is not a counter that was zero.
        let err = compare_counter_block("Read Record Count: 0\n", "boom\n").unwrap_err();
        assert!(err.contains("MET-006"), "{err}");
    }

    #[test]
    fn tst_003_a_cqlsh_result_set_parses_into_columns_sorted_rows_and_a_count() {
        let text = "
 key  | value
------+--------
 key1 | valueA
 key3 | valueC
 key2 | valueB

(3 rows)
";
        let tables = parse_cqlsh(text);
        assert_eq!(tables.len(), 1);
        assert_eq!(tables[0].header, vec!["key", "value"]);
        assert_eq!(
            tables[0].rows,
            vec!["key1 | valueA", "key2 | valueB", "key3 | valueC"]
        );
        assert_eq!(tables[0].row_count, Some(3));
    }

    #[test]
    fn tst_003_two_result_sets_in_one_transcript_stay_separate() {
        let text = "
 key
-----
 a

(1 rows)

 other
-------
 b

(1 rows)
";
        assert_eq!(parse_cqlsh(text).len(), 2);
    }

    #[test]
    fn tst_003_row_order_is_not_asserted_because_it_is_token_order() {
        // Java's `smoke/02` fixture records `key1, key3, key2`, which is the murmur3 order of
        // those three keys and nothing to do with the migration.
        let a = " key\n-----\n key1\n key3\n key2\n\n(3 rows)\n";
        let b = " key\n-----\n key1\n key2\n key3\n\n(3 rows)\n";
        assert!(compare_cqlsh(a, b).is_ok());
    }

    #[test]
    fn tst_003_a_missing_row_is_reported_with_both_row_sets() {
        let a = " key\n-----\n key1\n key2\n\n(2 rows)\n";
        let b = " key\n-----\n key1\n\n(1 rows)\n";
        let err = compare_cqlsh(a, b).unwrap_err();
        assert!(err.contains("rows"), "{err}");
    }

    #[test]
    fn tst_003_differing_columns_are_reported_before_the_rows_are_compared() {
        let a = " key | value\n-----+-------\n key1 | v\n\n(1 rows)\n";
        let b = " key | other\n-----+-------\n key1 | v\n\n(1 rows)\n";
        let err = compare_cqlsh(a, b).unwrap_err();
        assert!(err.contains("columns differ"), "{err}");
    }

    #[test]
    fn tst_003_a_script_is_flattened_without_losing_a_value_that_looks_like_a_comment() {
        // `'A---'` is a real value in smoke/03_ttl_writetime; a naive comment strip eats it.
        let script = "\
-- a leading comment
INSERT INTO t(k,v) VALUES ('r','A---');   -- a trailing one

INSERT INTO t(k,v) VALUES ('s','B---');
";
        assert_eq!(
            flatten_cql(script),
            "INSERT INTO t(k,v) VALUES ('r','A---');\nINSERT INTO t(k,v) VALUES ('s','B---');"
        );
    }

    #[test]
    fn tst_003_a_long_script_is_chunked_on_statement_boundaries() {
        let statement = |n: usize| format!("INSERT INTO t(k) VALUES ('{}');", "x".repeat(n));
        let script = std::iter::repeat_n(statement(1000), 200)
            .collect::<Vec<_>>()
            .join("\n");
        let chunks = chunk_cql(&script);
        assert!(chunks.len() > 1, "a 200 KB script must be split");
        for chunk in &chunks {
            assert!(chunk.len() <= MAX_SCRIPT_BYTES + 1100, "chunk too large");
            assert!(chunk.trim_end().ends_with(';'), "chunk cut mid-statement");
        }
        assert_eq!(
            chunks
                .iter()
                .map(|c| c.matches("INSERT").count())
                .sum::<usize>(),
            200,
            "chunking must not lose a statement"
        );
    }

    #[test]
    fn tst_003_a_semicolon_inside_a_literal_does_not_end_a_statement() {
        let chunks = chunk_cql("INSERT INTO t(k) VALUES ('a;b'); SELECT * FROM t;");
        assert_eq!(chunks.len(), 1);
        assert_eq!(
            split_statements("INSERT INTO t(k) VALUES ('a;b'); SELECT * FROM t;").len(),
            2
        );
    }

    #[test]
    fn tst_003_a_blocked_case_says_why_and_the_others_say_nothing() {
        // No case is blocked today: the last three were unblocked when validate learned to explode
        // a record and look one target row up per map entry. The parser and this check stay,
        // because a marker is how the *next* ported case says what it waits on — and a marker
        // makes its case report `ok` without running, so an unexplained one is worse than none.
        for case in cases().expect("tests/sit must be walkable") {
            if let Some(reason) = case.blocked() {
                assert!(
                    reason.len() > 40,
                    "{}'s `blocked` line does not explain anything: {reason}",
                    case.id()
                );
            }
        }
    }

    #[test]
    fn tst_003_a_job_name_round_trips() {
        for job in [SitJob::Migrate, SitJob::Validate, SitJob::Guardrail] {
            assert_eq!(SitJob::parse(job.as_str()).unwrap(), job);
        }
        assert!(SitJob::parse("DiffData").is_err());
    }
}
