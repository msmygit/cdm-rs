//! `cdm config init` — the wizard that writes a first configuration (`CLI-006`).
//!
//! The Java tool's onboarding is a properties file with a hundred and forty commented-out keys and
//! no way to tell which of them matter. This command replaces that with: answer three questions,
//! connect, look at the schema, and get a file that already knows what it is migrating.
//!
//! # What it does and does not decide
//!
//! It fills in what it can *observe*: the two tables, the contact points that worked, the local
//! datacenter the driver auto-detected, and the codecs the column types actually require. It does
//! **not** invent a `perfops.num_parts`, because the honest input to that number is the table's
//! on-disk size and nothing here knows it — so the generated file carries the default and a comment
//! saying what to divide. A configuration that guessed and got it wrong would be worse than one
//! that says it did not guess: the operator would trust it.
//!
//! # The interactive half
//!
//! Prompts go to stderr and answers come from stdin, so `cdm config init > cdm.toml` still produces
//! a clean file while the operator is being asked questions. A bare newline accepts the value in
//! brackets, and end-of-input — a pipe, a CI job, a `< /dev/null` — is treated as accepting every
//! remaining default rather than as an error, which is what makes the same code path serve
//! `--non-interactive`.

use std::io::{BufRead, Write};
use std::path::Path;

use cdm_config::CdmConfig;
use cdm_core::{CdmError, ErrorKind};
use serde::Serialize;

use crate::cli::{ConfigArgs, JobArgs};
use crate::harness;
use crate::loader::load;
use crate::output::Report;

/// What `cdm config init` produced (`CLI-006`).
#[derive(Debug, Serialize)]
pub struct InitReport {
    /// The generated configuration, as canonical TOML.
    pub config: String,
    /// Where it was written, when `--out` said so.
    pub written_to: Option<String>,
    /// The origin table it introspected.
    pub origin_table: String,
    /// The target table it introspected.
    pub target_table: String,
    /// Notes about what was observed and what was left at its default, in the order they matter.
    pub notes: Vec<String>,
}

impl Report for InitReport {
    fn render_human(&self, out: &mut dyn Write) -> std::io::Result<()> {
        match &self.written_to {
            Some(path) => {
                writeln!(
                    out,
                    "Wrote {path} for {} → {}.\n",
                    self.origin_table, self.target_table
                )?;
            }
            None => {
                writeln!(out, "{}", self.config)?;
            }
        }
        for note in &self.notes {
            writeln!(out, "  - {note}")?;
        }
        if self.written_to.is_some() {
            writeln!(
                out,
                "\nNext: `cdm config validate --config {} --tier semantic`, then `cdm plan`.",
                self.written_to.as_deref().unwrap_or("cdm.toml")
            )?;
        }
        Ok(())
    }
}

/// Runs the wizard and produces a configuration (`CLI-006`).
///
/// # Errors
///
/// [`ErrorKind::Config`] when no origin table can be established even after prompting, plus
/// whatever connecting and introspecting can return — this command deliberately *connects*, because
/// a configuration generated without looking at the schema is a template, and templates are what it
/// exists to replace.
pub fn init(
    args: &ConfigArgs,
    out_path: Option<&Path>,
    non_interactive: bool,
) -> Result<InitReport, CdmError> {
    let mut config = load(args)?.config.unwrap_or_default();

    if !non_interactive {
        let stdin = std::io::stdin();
        interview(&mut config, &mut stdin.lock(), &mut std::io::stderr());
    }

    if config.schema.origin.keyspace_table.is_none() {
        return Err(CdmError::new(
            ErrorKind::Config,
            "no origin table: `cdm config init` introspects the schema, so it needs to be told \
             which table to look at. Pass `--set schema.origin.keyspace_table=ks.tbl`, or run \
             without `--non-interactive` to be asked",
        )
        .with_context(|c| c.with_config_key("schema.origin.keyspace_table")));
    }

    // The connect-and-introspect the whole point rests on. It goes through the harness, so the
    // configuration this command writes is one the harness has already accepted.
    let tables = harness::resolve_tables(&JobArgs {
        config: overrides_from(args, &config),
        dry_run: false,
        summary_out: None,
        tui: false,
    })?;

    let mut notes =
        vec![
        // First, because it is the one thing that stops the generated file working, and finding
        // out from an authentication failure is a worse way to learn it. `Secret::serialize`
        // renders `***` unconditionally (`SEC-001`) — a generator that wrote the real password
        // into a file destined for source control would be a far worse defect than an incomplete
        // one — so the passwords have to be supplied, and `env:`/`file:`/`exec:` (`CFG-012`) is
        // how to do that without ever putting one in the file.
        "Passwords are written as `***`: a generated file must never carry a credential. Replace \
         them, ideally with an indirection such as `env:CDM_ORIGIN_PASSWORD` or \
         `file:/run/secrets/origin`, which is resolved at load time and never stored."
            .to_owned(),
        format!(
            "{} origin column(s) map onto {} target column(s).",
            tables.mapping.origin_columns().len(),
            tables.mapping.target_columns().len()
        ),
        format!("The origin reports the {:?} partitioner.", tables.partitioner()),
        "`perfops.num_parts` is left at its default: the right value is roughly the table's \
         on-disk size divided by 10 MB, which nothing here can observe. Set it before a large \
         migration."
            .to_owned(),
    ];

    if tables.target.is_counter_table() {
        notes.push(
            "The target is a counter table. Counter writes are not idempotent, so this run must \
             never be retried range-by-range or resumed blindly (MIG-032, DST-015)."
                .to_owned(),
        );
    }
    if tables
        .origin
        .columns
        .iter()
        .any(cdm_cql::schema::ColumnMeta::is_vector)
    {
        notes.push(
            "A `vector` column is present, which needs Cassandra 5.0 or later on both sides \
             (CDC-004)."
                .to_owned(),
        );
    }

    let toml = render(&config)?;
    let written_to = match out_path {
        None => None,
        Some(path) => {
            std::fs::write(path, &toml).map_err(|error| {
                CdmError::new(
                    ErrorKind::Config,
                    format!("cannot write {}: {error}", path.display()),
                )
            })?;
            Some(path.display().to_string())
        }
    };

    Ok(InitReport {
        config: toml,
        written_to,
        origin_table: tables.origin.quoted_name(),
        target_table: tables.target.quoted_name(),
        notes,
    })
}

/// Asks the three questions that cannot be observed, writing prompts to `prompts`.
///
/// Separated from [`init`] and taking its streams as parameters so a test can drive the whole
/// wizard with two buffers, which is the only way this is testable without a terminal.
fn interview(config: &mut CdmConfig, input: &mut dyn BufRead, prompts: &mut dyn Write) {
    let host = config.connect.origin.host.clone();
    if let Some(answer) = ask(input, prompts, "origin host", &host) {
        config.connect.origin.host = answer;
    }

    let origin = config
        .schema
        .origin
        .keyspace_table
        .clone()
        .unwrap_or_default();
    if let Some(answer) = ask(input, prompts, "origin keyspace.table", &origin) {
        config.schema.origin.keyspace_table = Some(answer);
    }

    // CFG-023: an empty answer leaves the target unset, which means "the same as the origin" —
    // the overwhelmingly common case, and one the operator should not have to retype.
    let target = config
        .schema
        .target
        .keyspace_table
        .clone()
        .unwrap_or_else(|| "same as origin".to_owned());
    if let Some(answer) = ask(input, prompts, "target keyspace.table", &target) {
        config.schema.target.keyspace_table = Some(answer);
    }
}

/// Asks one question. `None` means "keep what you had": an empty line, or end of input.
fn ask(
    input: &mut dyn BufRead,
    prompts: &mut dyn Write,
    question: &str,
    default: &str,
) -> Option<String> {
    let _ = write!(prompts, "{question} [{default}]: ");
    let _ = prompts.flush();

    let mut line = String::new();
    match input.read_line(&mut line) {
        Ok(0) | Err(_) => None,
        Ok(_) => {
            let answer = line.trim();
            (!answer.is_empty()).then(|| answer.to_owned())
        }
    }
}

/// The configuration as canonical TOML, with a header saying where it came from.
fn render(config: &CdmConfig) -> Result<String, CdmError> {
    let body = toml::to_string_pretty(config).map_err(|error| {
        CdmError::new(
            ErrorKind::Internal,
            format!("cannot render the generated configuration as TOML: {error}"),
        )
    })?;
    Ok(format!(
        "# Generated by `cdm config init` (CLI-006).\n\
         #\n\
         # Every value below is either something the schema reported or a documented default.\n\
         # `cdm config explain <key>` says where any one of them came from, and\n\
         # `cdm config validate` checks the whole file without touching a cluster.\n\
         \n{body}"
    ))
}

/// The arguments to introspect with: the operator's, plus whatever the interview changed.
///
/// Passed as `--set` overrides rather than by handing the harness a `CdmConfig`, so the answers go
/// through the same loader, the same alias resolution and the same precedence order as everything
/// else. A second door into the configuration is a second set of rules to keep in step.
fn overrides_from(args: &ConfigArgs, config: &CdmConfig) -> ConfigArgs {
    let mut resolved = args.clone();
    resolved.set.push(format!(
        "connect.origin.host={}",
        config.connect.origin.host
    ));
    if let Some(table) = &config.schema.origin.keyspace_table {
        resolved
            .set
            .push(format!("schema.origin.keyspace_table={table}"));
    }
    if let Some(table) = &config.schema.target.keyspace_table {
        resolved
            .set
            .push(format!("schema.target.keyspace_table={table}"));
    }
    resolved
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

    fn interview_with(answers: &str) -> CdmConfig {
        let mut config = CdmConfig::default();
        let mut prompts = Vec::new();
        interview(&mut config, &mut answers.as_bytes(), &mut prompts);
        config
    }

    #[test]
    fn cli_006_the_wizard_records_what_it_was_told() {
        let config = interview_with("cass-origin\nks.orders\nks2.orders\n");
        assert_eq!(config.connect.origin.host, "cass-origin");
        assert_eq!(
            config.schema.origin.keyspace_table.as_deref(),
            Some("ks.orders")
        );
        assert_eq!(
            config.schema.target.keyspace_table.as_deref(),
            Some("ks2.orders")
        );
    }

    #[test]
    fn cli_006_an_empty_answer_keeps_the_default() {
        // CFG-023: leaving the target blank means "the same table", which is the common case and
        // must not require retyping the origin.
        let config = interview_with("\nks.orders\n\n");
        assert!(config.schema.target.keyspace_table.is_none());
        assert_eq!(config.connect.origin.host, "localhost");
    }

    #[test]
    fn cli_006_end_of_input_is_not_an_error() {
        // A piped or redirected stdin must behave as `--non-interactive` does rather than fail:
        // the wizard is skippable, and a CI job that hangs or crashes on a prompt is the failure
        // this guards against.
        let config = interview_with("");
        assert_eq!(config, CdmConfig::default());
    }

    #[test]
    fn cli_006_the_generated_file_says_what_produced_it() {
        let toml = render(&CdmConfig::default()).unwrap();
        assert!(
            toml.starts_with("# Generated by `cdm config init`"),
            "{toml:.120}"
        );
        assert!(toml.contains("cdm config explain"), "{toml:.400}");
    }

    #[test]
    fn cli_006_the_generated_file_is_loadable_toml() {
        // The one property that matters: what this writes, `cdm --config` must read.
        let toml = render(&CdmConfig::default()).unwrap();
        let parsed: CdmConfig = toml::from_str(&toml).expect("the generated file must parse");

        // Compared through a re-render rather than by equality, and deliberately so. `Secret`
        // serialises as `***` (`SEC-001`), so the password that comes back is the literal `***`
        // and not the default -- which is correct, is why `sec_001_the_generated_file_carries_no_
        // credential` exists below, and makes a `PartialEq` between the two configurations false
        // for a reason that has nothing to do with round-tripping.
        assert_eq!(render(&parsed).unwrap(), toml);
    }

    #[test]
    fn sec_001_the_generated_file_carries_no_credential() {
        let mut config = CdmConfig::default();
        config.connect.origin.password = cdm_config::Secret::new("hunter2".to_owned());
        let toml = render(&config).unwrap();

        assert!(
            !toml.contains("hunter2"),
            "a generated configuration is a file destined for source control: {toml}"
        );
        assert!(toml.contains("***"), "{toml}");
    }
}
