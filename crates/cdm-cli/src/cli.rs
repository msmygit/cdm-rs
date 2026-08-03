//! Command-line surface (`CLI-001`).
//!
//! The subcommand tree mirrors `docs/SPEC.md` §16.1. Commands whose implementation has not landed
//! yet are present but return an error naming the pull request that delivers them — a missing
//! subcommand reads as "cdm-rs cannot do this", whereas a named one reads as "not yet", which is
//! the truth and is far more useful to someone evaluating the tool.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

/// Migrate and validate data between Cassandra-compatible clusters.
#[derive(Debug, Parser)]
#[command(name = "cdm", version, about, long_about = None, propagate_version = true)]
pub struct Cli {
    /// How to render output (`CLI-005`).
    ///
    /// Long-form only: `-o` is reserved for output *files*, per long-standing Unix convention and
    /// the `cdm config init -o cdm.toml` shape the README documents.
    #[arg(long, global = true, value_enum, default_value_t = OutputFormat::Human)]
    pub output: OutputFormat,

    /// Log verbosity.
    #[arg(long, global = true, default_value = "info")]
    pub log_level: String,

    /// Reproduce Java CDM behaviours that cdm-rs otherwise improves on.
    ///
    /// See `docs/MIGRATION_FROM_JAVA.md`. Two Java defects are deliberately **not** restored by
    /// this flag: the unreachable flush threshold and the always-zero validate error count.
    #[arg(long, global = true)]
    pub compat_java: bool,

    /// The command to run.
    #[command(subcommand)]
    pub command: Command,
}

/// Output rendering (`CLI-005`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat {
    /// Prose and tables for a terminal.
    Human,
    /// A single JSON document, for scripts and pipelines.
    Json,
}

/// Top-level commands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run a migration.
    Migrate(JobArgs),
    /// Compare origin and target, optionally correcting differences.
    #[command(alias = "diff")]
    Validate(JobArgs),
    /// Report oversized columns on the origin.
    Guardrail(JobArgs),
    /// Compute the token-range plan without touching data.
    Plan(JobArgs),
    /// Inspect and manage runs.
    Runs {
        /// The run operation.
        #[command(subcommand)]
        command: RunsCommand,
    },
    /// Work with configuration.
    Config {
        /// The configuration operation.
        #[command(subcommand)]
        command: ConfigCommand,
    },
    /// Inspect origin and target schemas.
    Schema {
        /// The schema operation.
        #[command(subcommand)]
        command: SchemaCommand,
    },
    /// Check connectivity to a cluster.
    Connect {
        /// The connectivity operation.
        #[command(subcommand)]
        command: ConnectCommand,
    },
    /// List registered codecs.
    Codecs,
    /// Show distributed-run membership.
    Cluster,
    /// Serve the control plane, web UI and metrics.
    Serve(ServeArgs),
    /// Serve the Model Context Protocol over stdio.
    Mcp,
    /// Print build information.
    Version,
    /// Generate a shell completion script (`CLI-007`).
    Completions {
        /// The shell to generate for.
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
}

/// Arguments shared by every command that loads a configuration.
///
/// The Java spellings (`--properties-file`, `--conf`) are accepted verbatim so an existing
/// invocation can be moved over by changing only the program name (`CLI-002`).
#[derive(Debug, Args, Clone, Default)]
pub struct ConfigArgs {
    /// Configuration file: TOML, YAML, JSON, or a Java `.properties` file.
    #[arg(long, short = 'c', value_name = "PATH")]
    pub config: Option<PathBuf>,

    /// A Java CDM properties file (`CLI-002`).
    #[arg(long, value_name = "PATH")]
    pub properties_file: Option<PathBuf>,

    /// Set a value using its canonical name, e.g. `--set perfops.num_parts=5000`.
    #[arg(long = "set", value_name = "KEY=VALUE")]
    pub set: Vec<String>,

    /// Set a value using its Java name, e.g. `--conf spark.cdm.perfops.numParts=5000`.
    #[arg(long = "conf", value_name = "KEY=VALUE")]
    pub conf: Vec<String>,

    /// Apply a named profile from the configuration file.
    #[arg(long, value_name = "NAME")]
    pub profile: Option<String>,

    /// Reject unknown `spark.cdm.*` keys instead of warning.
    #[arg(long)]
    pub strict_config: bool,
}

/// Arguments for the three job commands.
#[derive(Debug, Args, Clone)]
pub struct JobArgs {
    /// Where the configuration comes from.
    #[command(flatten)]
    pub config: ConfigArgs,

    /// Read and transform everything, but issue no target writes.
    #[arg(long)]
    pub dry_run: bool,

    /// Write a machine-readable run summary here when the run ends.
    #[arg(long, value_name = "PATH")]
    pub summary_out: Option<PathBuf>,
}

/// `cdm runs …`
#[derive(Debug, Subcommand)]
pub enum RunsCommand {
    /// List previous runs.
    List(ConfigArgs),
    /// Show one run in detail.
    Show {
        /// The run identifier.
        run_id: i64,
        /// Where the configuration comes from.
        #[command(flatten)]
        config: ConfigArgs,
    },
    /// Re-run the ranges a previous run did not finish.
    Resume {
        /// Adopt the most recent unfinished run automatically.
        #[arg(long)]
        auto: bool,
        /// Where the configuration comes from.
        #[command(flatten)]
        config: ConfigArgs,
    },
    /// Cancel a running run.
    Cancel {
        /// The run identifier.
        run_id: i64,
        /// Where the configuration comes from.
        #[command(flatten)]
        config: ConfigArgs,
    },
}

/// `cdm config …`
#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    /// Generate a configuration by introspecting the schema (`CLI-006`).
    Init {
        /// Where the configuration comes from.
        #[command(flatten)]
        config: ConfigArgs,
        /// Where to write the generated configuration.
        #[arg(long, short = 'o', value_name = "PATH")]
        out: Option<PathBuf>,
        /// Do not prompt; use defaults and supplied flags only.
        #[arg(long)]
        non_interactive: bool,
    },
    /// Validate a configuration (`CFG-020`).
    Validate {
        /// Where the configuration comes from.
        #[command(flatten)]
        config: ConfigArgs,
        /// The highest tier to run. Tier 3 needs cluster access.
        #[arg(long, value_enum, default_value_t = Tier::Semantic)]
        tier: Tier,
    },
    /// Explain one property: its value, and where that value came from (`CFG-028`).
    Explain {
        /// The canonical or Java property name.
        key: String,
        /// Where the configuration comes from.
        #[command(flatten)]
        config: ConfigArgs,
    },
    /// Compare two configurations, ignoring ordering and defaults (`CFG-029`).
    Diff {
        /// The baseline configuration.
        left: PathBuf,
        /// The configuration to compare against it.
        right: PathBuf,
    },
    /// Convert a Java properties file to canonical form (`CLI-003`).
    Convert {
        /// The Java properties file to read.
        #[arg(long, value_name = "PATH")]
        from: PathBuf,
        /// Where to write the converted configuration; stdout when omitted.
        #[arg(long, value_name = "PATH")]
        to: Option<PathBuf>,
    },
    /// Print the JSON Schema of the configuration model (`CFG-003`).
    Schema,
}

/// How far configuration validation should go (`CFG-020`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Tier {
    /// Types, ranges and enumeration values.
    Syntactic,
    /// Everything above, plus cross-property rules.
    Semantic,
    /// Everything above, plus checks against the live schema. Requires cluster access.
    Schema,
}

/// `cdm schema …`
#[derive(Debug, Subcommand)]
pub enum SchemaCommand {
    /// Print a table's schema.
    Show(ConfigArgs),
    /// Compare origin and target schemas, with the conversion plan (`SCH-008`).
    Diff(ConfigArgs),
}

/// `cdm connect …`
#[derive(Debug, Subcommand)]
pub enum ConnectCommand {
    /// Connect and report what was negotiated (`CON-008`, `CON-029`).
    Test {
        /// Which side to test.
        #[arg(long, value_enum, default_value_t = SideArg::Both)]
        side: SideArg,
        /// Where the configuration comes from.
        #[command(flatten)]
        config: ConfigArgs,
    },
}

/// Which cluster a command applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum SideArg {
    /// The cluster being read from.
    Origin,
    /// The cluster being written to.
    Target,
    /// Both clusters.
    Both,
}

/// `cdm serve`
#[derive(Debug, Args, Clone)]
pub struct ServeArgs {
    /// Where the configuration comes from.
    #[command(flatten)]
    pub config: ConfigArgs,

    /// Address to bind the control plane to.
    #[arg(long, value_name = "ADDR")]
    pub bind: Option<String>,
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
mod tests {
    use clap::CommandFactory;

    use super::*;

    #[test]
    fn cli_001_the_command_tree_is_well_formed() {
        // clap's own assertions catch conflicting flags, duplicate short options and the like.
        Cli::command().debug_assert();
    }

    #[test]
    fn cli_002_java_invocation_shapes_parse() {
        // The whole point of CLI-002: this line differs from a Java invocation only in the
        // program name.
        let cli = Cli::try_parse_from([
            "cdm",
            "migrate",
            "--properties-file",
            "cdm.properties",
            "--conf",
            "spark.cdm.schema.origin.keyspaceTable=ks.tbl",
        ])
        .expect("Java flag spellings must parse");

        let Command::Migrate(args) = cli.command else {
            panic!("expected migrate")
        };
        assert_eq!(
            args.config.properties_file,
            Some(PathBuf::from("cdm.properties"))
        );
        assert_eq!(
            args.config.conf,
            vec!["spark.cdm.schema.origin.keyspaceTable=ks.tbl"]
        );
    }

    #[test]
    fn cli_002_conf_is_repeatable() {
        let cli = Cli::try_parse_from([
            "cdm",
            "validate",
            "--conf",
            "spark.cdm.perfops.numParts=100",
            "--conf",
            "spark.cdm.perfops.batchSize=1",
        ])
        .unwrap();
        let Command::Validate(args) = cli.command else {
            panic!("expected validate")
        };
        assert_eq!(args.config.conf.len(), 2);
    }

    #[test]
    fn cli_001_diff_is_an_alias_for_validate() {
        // Java users know the job as DiffData; the alias saves them looking it up.
        let cli = Cli::try_parse_from(["cdm", "diff", "--config", "cdm.toml"]).unwrap();
        assert!(matches!(cli.command, Command::Validate(_)));
    }

    #[test]
    fn cli_005_output_format_is_global() {
        let cli = Cli::try_parse_from(["cdm", "--output", "json", "codecs"]).unwrap();
        assert_eq!(cli.output, OutputFormat::Json);

        // Also accepted after the subcommand, which is where people naturally type it.
        let cli = Cli::try_parse_from(["cdm", "codecs", "--output", "json"]).unwrap();
        assert_eq!(cli.output, OutputFormat::Json);
    }

    #[test]
    fn cli_001_validation_defaults_to_the_tier_that_needs_no_cluster() {
        let cli = Cli::try_parse_from(["cdm", "config", "validate"]).unwrap();
        let Command::Config {
            command: ConfigCommand::Validate { tier, .. },
        } = cli.command
        else {
            panic!("expected config validate")
        };
        assert_eq!(
            tier,
            Tier::Semantic,
            "the default must not silently require credentials"
        );
    }
}
