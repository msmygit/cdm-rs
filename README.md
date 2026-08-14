# gh-pages — benchmark baseline series

This branch is **not** the documentation site. `docs.yml` publishes that via
`actions/deploy-pages` from a build artefact, so nothing here is served as the project page.

What lives here is the time series written by `benchmark-action/github-action-benchmark`
under `dev/bench/`, which `bench.yml` compares each run against (`TST-060`).

Created as an empty orphan commit because the action fetches this branch before it can create
it, and so cannot bootstrap itself on a repository that has never had one.

Nothing here is hand-edited.
