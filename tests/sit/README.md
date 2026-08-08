# The ported Java SIT parity suite (`TST-003`)

Nineteen cases, one directory each, in the same three phases the Java tree uses. Each directory
holds a `case.txt` — the step list that replaces Java's `cdm.txt` and `execute.sh` — plus the CQL
scripts, `.properties` files, counter-block expectations and final-state expectations the steps
name. `crates/cdm-testkit/src/sit.rs` parses them; `crates/cdm-testkit/tests/sit_it.rs` runs them
against a container; `cargo xtask sit` is the one command that does both.

```
cql   <file>                              run a CQL script (Java: setup.cql / breakData.cql)
job   <migrate|validate|guardrail> <properties> <assert>
check <query.cql> <expected.out>          assert the final state of the target
```

`.properties` files are templates: `{{host}}` and `{{port}}` are substituted with the fixture's
contact point, because the Java originals hard-code the container name `cdm-sit-cass` and a
container's published port is chosen at run time.

## The expectations are regenerated, not ported

Java's `.assert` and `expected.out` files record what Java does, and cdm-rs deliberately does not
do all of it. Copying them across would have turned two documented Java defects into cdm-rs's
expected behaviour:

* **`MIG-004` / divergence 15** — Java compares the *committed* `UNFLUSHED` against a threshold
  that is only ever incremented at the interim level, so it flushes once per range and buffers
  everything. cdm-rs flushes at the documented threshold.
* **`ENG-008` / divergence 16** — Java reads a failed validate range's terms at the committed
  level, where they are all still zero, so a failed range always reports `Error Record Count: 0`.
  cdm-rs reports the real count.

Neither is visible in a *passing* case's counter block, which is exactly why they are dangerous to
port by hand: the fixture would look right and encode the bug. Every expectation here was derived
from `docs/SPEC.md` first and confirmed against a run second. Where a value differs from the Java
original, the case file says which numbered divergence in `docs/MIGRATION_FROM_JAVA.md` explains
it.

Row **order** is not asserted. Java's `expected.out` files record the order `cqlsh` returned, which
for a partition-key scan is murmur3 token order — a fact about the fixture's keys rather than about
the migration. The harness compares the sorted row set, the column list and the `(N rows)` count.

## Cases that cannot run yet

Three cases carry a `blocked <reason>` line in their `case.txt`, which the runner prints instead of
asserting. All three are the same gap: validate issues one target lookup per *record*, where an
explode map produces one target row per map *entry*, so every entry reports missing. The cases are
written in full and will pass unchanged once that work lands.
