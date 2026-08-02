## What does this PR do?

<!-- One paragraph. What changed and why. -->

## Requirements

<!-- Every PR maps to specification IDs. See docs/SPEC.md and docs/TRACEABILITY.md. -->

Implements: <!-- e.g. MIG-012, MIG-013 -->
Roadmap PR: <!-- e.g. #21 in docs/ROADMAP.md -->

## Checklist

- [ ] `Implements:` trailer present on the commit(s)
- [ ] `docs/TRACEABILITY.md` updated for every requirement touched
- [ ] Unit tests added, named `<req_id>_<description>`
- [ ] Integration or SIT test added where cluster behaviour changed
- [ ] Rustdoc written for every new public item
- [ ] Generated artefacts refreshed (`cargo xtask openapi`, `cargo xtask docs`)
- [ ] `docs/MIGRATION_FROM_JAVA.md` updated if behaviour differs from Java CDM
- [ ] Benchmarks run if the hot path changed (no regression > 10%)
- [ ] No new Clippy allowances without a justifying comment

## Behaviour changes vs Java CDM

<!-- None, or: what changed, why, and the --compat-java flag that restores the old behaviour. -->

## How was this verified?

<!-- Commands run, clusters used, output. -->
