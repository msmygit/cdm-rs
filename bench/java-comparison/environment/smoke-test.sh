#!/usr/bin/env bash
#
# Prove the tier-3 environment actually moves rows (`NFR-004`, `TST-060`).
#
# This is not the benchmark. It is the much cheaper question the benchmark depends on: does Java
# CDM 6.0.1 on Spark 4.1.2, in this image, against these two containers, read rows from origin and
# write them to target? A version mismatch in this stack does not fail at startup -- Spark starts,
# the job is accepted, and it dies with `NoSuchMethodError: scala.runtime.Statics.releaseFence()`
# on the first RDD touch. Only an end-to-end run that counts rows on the far side answers it, so
# that is what this does, on a dataset small enough to run in a couple of minutes.
#
# Usage:
#   bench/java-comparison/environment/smoke-test.sh [row_count]      # default 10000
#
# Assumes `build-image.sh` and `clusters-up.sh` have run. Leaves the clusters up so the tables can
# be inspected; run `clusters-down.sh` when finished. Exit status is 0 only if the target row
# count equals the origin row count and CDM reported no errors.

set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=versions.env
. "${HERE}/versions.env"

ROWS="${1:-10000}"
KEYSPACE=cdm_smoke
TABLE=orders
KS_TABLE="${KEYSPACE}.${TABLE}"

log() { printf '[smoke] %s\n' "$*"; }

# Checked up front rather than discovered three steps in as `Error response from daemon: No such
# container`. On a shared Docker daemon these containers are easily removed by someone else's
# `docker system prune`, which is exactly how this check came to be written.
for required in "${ORIGIN_CONTAINER}" "${TARGET_CONTAINER}"; do
    if ! docker ps --format '{{.Names}}' | grep -qx "${required}"; then
        log "${required} is not running; run clusters-up.sh first"
        exit 2
    fi
done
if ! docker image inspect "${BENCH_IMAGE}" > /dev/null 2>&1; then
    log "${BENCH_IMAGE} is not built; run build-image.sh first"
    exit 2
fi

# cqlsh runs inside the node's own container: right version, no host install, and it reaches the
# node over the same address the Spark driver will use, which incidentally proves that address is
# the one the node advertises.
cql_origin() { docker exec -i "${ORIGIN_CONTAINER}" cqlsh "${ORIGIN_IP}" "${NATIVE_PORT}" "$@"; }
cql_target() { docker exec -i "${TARGET_CONTAINER}" cqlsh "${TARGET_IP}" "${NATIVE_PORT}" "$@"; }

# NetworkTopologyStrategy, not SimpleStrategy. CDM reads and writes at LOCAL_QUORUM by default,
# and LOCAL_QUORUM is defined in terms of the replicas in the *local* datacenter; SimpleStrategy
# has no datacenter concept, and the combination is the sort of thing that works on one version
# and raises `UnavailableException` on another. `datacenter1` is what a stock `cassandra:5.0`
# container calls its DC.
DDL="CREATE KEYSPACE IF NOT EXISTS ${KEYSPACE}
     WITH replication = {'class':'NetworkTopologyStrategy','datacenter1':1};
     CREATE TABLE IF NOT EXISTS ${KS_TABLE} (
       id uuid PRIMARY KEY,
       customer text,
       amount   int,
       note     text
     );"

log "creating ${KS_TABLE} on both clusters"
cql_origin -e "${DDL}" > /dev/null
cql_target -e "${DDL}" > /dev/null

# Truncate rather than assume: a rerun against a target that still holds the previous run's rows
# would pass its row-count check without CDM having moved anything at all. That is precisely the
# false pass this script exists to rule out.
log "truncating both tables"
cql_origin -e "TRUNCATE ${KS_TABLE};" > /dev/null
cql_target -e "TRUNCATE ${KS_TABLE};" > /dev/null

# Loaded as a CSV via `cqlsh COPY ... FROM`, not as a stream of `INSERT` statements.
#
# The first version of this script generated `BEGIN UNLOGGED BATCH` blocks in `awk` and piped them
# into `cqlsh`. It worked at 2,000 rows and failed at 100,000, roughly 10,500 statements in, with
# `NoHostAvailable: Connection to 172.28.0.11:9042 was closed` -- a single cqlsh connection
# feeding a node batches as fast as bash can print them is a good way to trip a coordinator, and
# the failure is unrelated to anything being measured. `COPY FROM` is cqlsh's own bulk loader:
# multi-process, chunked, with its own retries, and it is what a Cassandra operator would use.
#
# The UUIDs are generated here rather than by CQL's `uuid()` so that the *same* keys can be
# reproduced; well-distributed partition keys matter because rows clustered into one token range
# would leave every ring split but one empty and the job would "succeed" while exercising nothing.
log "loading ${ROWS} rows into origin"
CSV="$(mktemp)"
awk -v n="${ROWS}" 'BEGIN {
    srand(20260814);          # fixed seed: the same dataset every run
    for (i = 1; i <= n; i++) {
        # A version-4 UUID assembled by hand. `uuidgen` is not in the container and calling it n
        # times from the shell is slower than the load it feeds.
        u = ""
        for (j = 0; j < 32; j++) {
            if (j == 12) c = "4";
            else if (j == 16) c = substr("89ab", int(rand() * 4) + 1, 1);
            else c = substr("0123456789abcdef", int(rand() * 16) + 1, 1);
            u = u c
            if (j == 7 || j == 11 || j == 15 || j == 19) u = u "-"
        }
        printf "%s,customer-%d,%d,row %d\n", u, i % 1000, (i * 7) % 100000, i
    }
}' > "${CSV}"
docker cp "${CSV}" "${ORIGIN_CONTAINER}:/tmp/smoke.csv" > /dev/null
rm -f "${CSV}"
cql_origin -e "COPY ${KS_TABLE} (id, customer, amount, note) FROM '/tmp/smoke.csv';" > /dev/null

origin_count() { cql_origin -e "SELECT COUNT(*) FROM ${KS_TABLE};" | awk 'NR==4 {print $1}'; }
target_count() { cql_target -e "SELECT COUNT(*) FROM ${KS_TABLE};" | awk 'NR==4 {print $1}'; }

ORIGIN_ROWS="$(origin_count)"
BEFORE="$(target_count)"
log "origin holds ${ORIGIN_ROWS} rows; target holds ${BEFORE} before migration"

if [ "${ORIGIN_ROWS}" != "${ROWS}" ]; then
    log "origin row count ${ORIGIN_ROWS} != requested ${ROWS}; the load failed, aborting"
    exit 1
fi

log "running Java CDM Migrate"
"${HERE}/submit-migrate.sh" "${KS_TABLE}"

AFTER="$(target_count)"
log "target holds ${AFTER} rows after migration (was ${BEFORE})"

if [ "${AFTER}" != "${ORIGIN_ROWS}" ]; then
    log "FAIL: target ${AFTER} != origin ${ORIGIN_ROWS}"
    exit 1
fi

log "PASS: Java CDM ${CDM_VERSION} on Spark ${SPARK_VERSION} migrated ${AFTER} rows"
