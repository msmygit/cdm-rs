//! `Secret<String>` and the `env:` / `file:` / `exec:` indirection forms (`CFG-012`, `SEC-001`).

use std::fmt;
use std::path::Path;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use zeroize::Zeroize;

use crate::meta::{PropertyKind, PropertyValue};

/// What every rendering of a secret shows instead of the secret.
pub const REDACTED: &str = "***";

/// A value that must never reach a log, a diagnostic, an API response or a terminal.
///
/// `Debug`, `Display` and `Serialize` all emit [`REDACTED`]. The only way to obtain the real
/// value is [`Secret::expose`], whose name is deliberately awkward so that a review notices it.
/// The buffer is zeroed on drop.
///
/// ```
/// use cdm_config::Secret;
///
/// let token: Secret<String> = Secret::new("AstraCS:hunter2");
/// assert_eq!(format!("{token}"), "***");
/// assert_eq!(format!("{token:?}"), "Secret(***)");
/// assert_eq!(serde_json::to_string(&token).unwrap(), "\"***\"");
/// assert_eq!(token.expose(), "AstraCS:hunter2");
/// ```
#[derive(Clone, PartialEq, Eq)]
pub struct Secret<T: Zeroize = String>(T);

impl<T: Zeroize> Secret<T> {
    /// Wraps a value.
    pub fn new(value: impl Into<T>) -> Self {
        Self(value.into())
    }

    /// Yields the protected value. Every call site is a `SEC-001` review point.
    pub fn expose(&self) -> &T {
        &self.0
    }
}

impl Secret<String> {
    /// Whether the value is empty, which `CFG-026` warns about.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl<T: Zeroize> Drop for Secret<T> {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl<T: Zeroize> fmt::Debug for Secret<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Secret({REDACTED})")
    }
}

impl<T: Zeroize> fmt::Display for Secret<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(REDACTED)
    }
}

impl<T: Zeroize> Serialize for Secret<T> {
    /// Always serialises [`REDACTED`]. A configuration that round-trips through JSON therefore
    /// loses its credentials by construction, which is the intended failure mode: `cdm-config`
    /// never re-serialises a loaded configuration, it merges untyped layers and deserialises
    /// once (`ARCHITECTURE.md` §4).
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(REDACTED)
    }
}

impl<'de> Deserialize<'de> for Secret<String> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer).map(Self)
    }
}

impl schemars::JsonSchema for Secret<String> {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Secret".into()
    }

    fn json_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "string",
            "format": "password",
            "description":
                "A credential. Accepts the literal value or the indirection forms `env:VAR`, \
                 `file:/path` and `exec:command` (CFG-012).",
        })
    }
}

impl PropertyValue for Secret<String> {
    fn kind() -> PropertyKind {
        PropertyKind::Secret
    }

    /// Overrides the blanket implementation, which would report `***`. Only built-in defaults —
    /// public constants such as Java CDM's `cassandra` — reach this method; user-supplied values
    /// never do. Callers must render [`PropertyMeta::displayed_default`](crate::meta::PropertyMeta::displayed_default)
    /// rather than this, which redacts based on the `secret` flag.
    fn display_value(&self) -> Option<String> {
        Some(self.0.clone())
    }
}

/// Something that can resolve the indirection forms of `CFG-012`.
///
/// Injectable so that tests, and one day a vault integration, can substitute for the process
/// environment, the filesystem and the shell.
pub trait SecretSource: fmt::Debug {
    /// Reads an environment variable.
    fn env(&self, name: &str) -> Result<String, String>;
    /// Reads a file, whose trailing newline is stripped.
    fn file(&self, path: &Path) -> Result<String, String>;
    /// Runs a command through the system shell and captures its standard output.
    fn exec(&self, command: &str) -> Result<String, String>;
}

/// The default [`SecretSource`]: the real environment, filesystem and shell.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemSecrets;

impl SecretSource for SystemSecrets {
    fn env(&self, name: &str) -> Result<String, String> {
        std::env::var(name).map_err(|_| format!("environment variable `{name}` is not set"))
    }

    fn file(&self, path: &Path) -> Result<String, String> {
        std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read `{}`: {e}", path.display()))
            .map(|text| text.trim_end_matches(['\n', '\r']).to_owned())
    }

    fn exec(&self, command: &str) -> Result<String, String> {
        let output = std::process::Command::new("sh")
            .arg("-c")
            .arg(command)
            .output()
            .map_err(|e| format!("cannot run the command: {e}"))?;
        if !output.status.success() {
            // The command line may itself name a secret store path, so it is echoed, but its
            // stderr is not: a failing `vault read` prints the token it was given.
            return Err(format!("the command exited with {}", output.status));
        }
        String::from_utf8(output.stdout)
            .map_err(|_| "the command produced output that is not UTF-8".to_owned())
            .map(|text| text.trim_end_matches(['\n', '\r']).to_owned())
    }
}

/// Resolves one raw secret value, following `env:` / `file:` / `exec:` indirection.
///
/// A value with no recognised prefix is the secret itself. The error is a plain sentence, ready
/// to become a [`Diagnostic`](cdm_core::Diagnostic) detail; it never contains the resolved value.
pub fn resolve(raw: &str, source: &dyn SecretSource) -> Result<String, String> {
    if let Some(name) = raw.strip_prefix("env:") {
        source.env(name.trim())
    } else if let Some(path) = raw.strip_prefix("file:") {
        source.file(Path::new(path.trim()))
    } else if let Some(command) = raw.strip_prefix("exec:") {
        source.exec(command.trim())
    } else {
        Ok(raw.to_owned())
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

    /// A struct holding a secret, to prove a *derived* `Debug` does not leak one either.
    #[derive(Debug)]
    struct Holder {
        #[allow(dead_code)]
        password: Secret<String>,
    }

    #[test]
    fn cfg_012_a_secret_redacts_in_every_rendering() {
        let secret: Secret<String> = Secret::new("hunter2");
        assert_eq!(secret.to_string(), "***");
        assert_eq!(format!("{secret:?}"), "Secret(***)");
        assert_eq!(serde_json::to_string(&secret).unwrap(), "\"***\"");
        assert_eq!(secret.expose(), "hunter2");
        assert!(!secret.is_empty());
        assert!(Secret::<String>::new("").is_empty());

        let rendered = format!(
            "{:?}",
            Holder {
                password: Secret::new("hunter2")
            }
        );
        assert!(!rendered.contains("hunter2"), "{rendered}");
    }

    #[test]
    fn cfg_012_a_secret_deserialises_from_a_plain_string() {
        let secret: Secret<String> = serde_json::from_str("\"hunter2\"").unwrap();
        assert_eq!(secret.expose(), "hunter2");
    }

    #[test]
    fn cfg_012_env_indirection_is_resolved_at_load_time() {
        // SAFETY-INVARIANT: this test owns a uniquely named variable, so no other test observes
        // the mutation. `set_var` is safe on this edition.
        std::env::set_var("CDM_TEST_SECRET_ENV_CASE", "from-env");
        assert_eq!(
            resolve("env:CDM_TEST_SECRET_ENV_CASE", &SystemSecrets).unwrap(),
            "from-env"
        );
        assert!(resolve("env:CDM_TEST_SECRET_ABSENT", &SystemSecrets)
            .unwrap_err()
            .contains("is not set"));
    }

    #[test]
    fn cfg_012_file_indirection_strips_the_trailing_newline() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(file, "from-file").unwrap();
        let spec = format!("file:{}", file.path().display());
        assert_eq!(resolve(&spec, &SystemSecrets).unwrap(), "from-file");
        assert!(resolve("file:/no/such/path", &SystemSecrets).is_err());
    }

    #[test]
    fn cfg_012_exec_indirection_captures_stdout() {
        assert_eq!(
            resolve("exec:printf from-exec", &SystemSecrets).unwrap(),
            "from-exec"
        );
        assert!(resolve("exec:exit 3", &SystemSecrets).is_err());
    }

    #[test]
    fn cfg_012_a_value_without_a_prefix_is_the_secret_itself() {
        assert_eq!(resolve("literal", &SystemSecrets).unwrap(), "literal");
        assert_eq!(
            resolve("AstraCS:token-with-a-colon", &SystemSecrets).unwrap(),
            "AstraCS:token-with-a-colon"
        );
    }
}
