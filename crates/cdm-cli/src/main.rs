//! The `cdm` binary: argument parsing, output rendering, exit codes.
//!
//! See [`docs/SPEC.md`](https://github.com/msmygit/cdm-rs/blob/main/docs/SPEC.md) `CLI-001`.

fn main() {
    println!(
        "cdm {} — scaffolding; see docs/ROADMAP.md",
        env!("CARGO_PKG_VERSION")
    );
}
