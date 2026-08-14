#!/usr/bin/env bash
#
# Run one Java CDM `Migrate` job against the containerised origin/target pair (`NFR-004`,
# `TST-060`). This is the tier-3 baseline command: whatever number it prints is the thing cdm-rs
# is being compared against, so the settings are argued for rather than inherited.
#
# Usage:
#   bench/java-comparison/environment/submit-migrate.sh <keyspace.table> [outdir]
#
# Environment overrides (all have defaults justified below):
#   CDM_CLASS         job class            default com.datastax.cdm.job.Migrate
#   DRIVER_MEMORY     Spark driver heap    default 4G
#   LOCAL_THREADS     local[N]             default 4
#   CDM_NUM_PARTS     ring splits          default 64
#   CDM_BATCH_SIZE    writes per batch     default 5
#   CDM_FETCH_SIZE    read page size       default 1000
#   CDM_RATELIMIT     rows/s cap per side  default 100000
#
# Writes `<outdir>/cdm-<timestamp>.log` (the full Spark/CDM output) and
# `<outdir>/cdm-<timestamp>.properties` (the exact configuration used), then prints CDM's own
# counter block and the wall-clock seconds. Exits non-zero if spark-submit failed OR if CDM
# reported any error records -- see the note above the check at the foot of this file, because CDM
# itself does not.
#
# ---------------------------------------------------------------------------------------------
# Why these Spark settings, and not the ones in CDM's README
#
# CDM's README shows `--master "local[*]" --driver-memory 25G --executor-memory 25G`. That is for
# a dedicated migration VM. The reference environment here is a free GitHub Actions
# `ubuntu-latest` runner: 4 vCPU, 16 GiB RAM, ~14 GiB free disk, and it is simultaneously hosting
# both Cassandra nodes. 25 GiB does not exist on it, and `--driver-memory 25G` on a 16 GiB box
# fails at JVM start or, worse, starts and then has the kernel OOM-kill a Cassandra container
# mid-run, which looks like a CDM bug and is not.
#
#   --master local[4]     One JVM, no cluster manager, no shuffle service. Spark's *standalone*
#                         mode on a single box would add a master and a worker process -- two more
#                         JVMs, ~500 MiB each, for no parallelism that `local[N]` does not already
#                         give.
#
#                         4, not `*`, and this was learned the hard way. On the reference runner
#                         they are the same thing, so pinning costs nothing there and makes a run
#                         on a larger developer box measure the same shape. It is also the
#                         difference between a correct run and a silently lossy one: at `local[*]`
#                         on a 12-core host, 200,000 rows produced 34,454 `Error Record Count` and
#                         1,465 missing rows on the target, with
#                         `NodeUnavailableException: No connection was available` -- 12 task
#                         threads firing unbounded async batches exhaust the Java driver's
#                         connection pool against a single-node target. See
#                         README-ENVIRONMENT.md, "Caveats". Raise it only alongside the target's
#                         capacity.
#
#   --driver-memory 4G    In `local` mode there are no executors: the driver JVM *is* the
#                         executor, and `--executor-memory` is read and then ignored. Passing it
#                         is cargo cult. 4 GiB is the budget that leaves room for two 1 GiB-heap
#                         Cassandra JVMs plus their off-heap and the page cache on 16 GiB -- see
#                         README-ENVIRONMENT.md, "RAM budget". CDM streams rows range by range and
#                         never collects to the driver, so 4 GiB is not a throughput constraint;
#                         it was verified against the smoke run recorded in that README.
#
#   spark.local.dir       Pointed at the bind-mounted output directory rather than the container's
#                         `/tmp`. Shuffle spill on a container's writable layer counts against
#                         Docker's storage, which on a 14 GiB runner is the disk that runs out.
#
#   spark.ui.enabled      Off. The Spark UI binds a port, starts a Jetty server and retains stage
#                         data for the run's lifetime. It is genuinely useful when diagnosing, and
#                         is pure overhead in a measurement. Set SPARK_UI=true to get it back.
#
#   --executor-memory     Deliberately absent. See --driver-memory above.

set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=versions.env
. "${HERE}/versions.env"

if [ $# -lt 1 ]; then
    sed -n '3,23p' "$0" | sed 's/^# \{0,1\}//'
    exit 2
fi

KEYSPACE_TABLE="$1"
OUTDIR="${2:-${HERE}/out}"

CDM_CLASS="${CDM_CLASS:-com.datastax.cdm.job.Migrate}"
DRIVER_MEMORY="${DRIVER_MEMORY:-4G}"
LOCAL_THREADS="${LOCAL_THREADS:-4}"
# 64, not CDM's default of 5000. On a single node each ring split is a Spark task with a driver
# round trip and a fresh set of prepared-statement bindings; at a few million rows, 5000 of them
# spend more time in the scheduler than in Cassandra. 64 is 16 tasks per core on a 4-vCPU box,
# which is enough to keep every core busy through a straggler and few enough that per-task cost
# stays under a percent. This is a tuning choice in Java CDM's favour and is called out in
# METHODOLOGY terms in README-ENVIRONMENT.md; raise it for larger datasets.
CDM_NUM_PARTS="${CDM_NUM_PARTS:-64}"
CDM_BATCH_SIZE="${CDM_BATCH_SIZE:-5}"
CDM_FETCH_SIZE="${CDM_FETCH_SIZE:-1000}"
# See the template: CDM's default of 20000 is a hard cap, not a target, so it cannot be left where
# it is for a throughput measurement. Nor can it simply be raised to infinity -- the limiter is
# also the only backpressure in CDM's write path, and at 1,000,000 the same 200,000-row job lost
# 135,264 records to connection-pool exhaustion. 100,000 is high enough to be inert on this
# hardware (measured steady state is ~18,000 rows/s) and low enough to still throttle a burst.
# Whatever dataset is finally chosen, confirm the measured rate is well under this number before
# believing any ratio computed from it.
CDM_RATELIMIT="${CDM_RATELIMIT:-100000}"
SPARK_UI="${SPARK_UI:-false}"

mkdir -p "${OUTDIR}"
STAMP="$(date +%Y%m%d_%H%M%S)"
PROPS="${OUTDIR}/cdm-${STAMP}.properties"
LOG="${OUTDIR}/cdm-${STAMP}.log"

# The properties file is generated per run and kept: a benchmark result whose configuration was
# overwritten by the next run is not reproducible, and `sed` here is cheaper than remembering.
sed \
    -e "s|@ORIGIN_IP@|${ORIGIN_IP}|g" \
    -e "s|@TARGET_IP@|${TARGET_IP}|g" \
    -e "s|@NATIVE_PORT@|${NATIVE_PORT}|g" \
    -e "s|@KEYSPACE_TABLE@|${KEYSPACE_TABLE}|g" \
    -e "s|@RATELIMIT@|${CDM_RATELIMIT}|g" \
    -e "s|@NUM_PARTS@|${CDM_NUM_PARTS}|g" \
    -e "s|@BATCH_SIZE@|${CDM_BATCH_SIZE}|g" \
    -e "s|@FETCH_SIZE@|${CDM_FETCH_SIZE}|g" \
    "${HERE}/cdm.properties.template" > "${PROPS}"

printf '[submit] %s %s\n' "${CDM_CLASS}" "${KEYSPACE_TABLE}"
printf '[submit] image %s\n' "${BENCH_IMAGE}"
printf '[submit] driver-memory %s, local[%s], numParts %s, ratelimit %s\n' \
    "${DRIVER_MEMORY}" "${LOCAL_THREADS}" "${CDM_NUM_PARTS}" "${CDM_RATELIMIT}"
printf '[submit] log %s\n' "${LOG}"

# A passwd database containing the invoking uid. See the `--user` note below: without this the JVM
# cannot resolve its own user and spark-submit fails twice over, in two unrelated-looking ways,
# before the job class is reached.
PASSWD="${OUTDIR}/passwd"
printf 'cdmbench:x:%s:%s::/work/out:/bin/bash\nroot:x:0:0::/root:/bin/bash\n' \
    "$(id -u)" "$(id -g)" > "${PASSWD}"

# `--user` so nothing lands in the output directory owned by root: this directory is on the host,
# and a root-owned log is one a subsequent unprivileged `run.sh` cannot rotate or delete.
#
# That then requires the generated `/etc/passwd` above, and it is not optional. Both failures were
# observed, in this order, on the first two attempts at this script:
#
#   1. `java.lang.IllegalArgumentException: basedir must be absolute: ?/.ivy2.5.2/local`
#      -- `getpwuid` fails, `user.home` becomes the literal string `?`, and spark-submit builds an
#      Ivy resolver from it before it ever looks at `--class`, whether or not `--packages` was
#      passed. Setting `HOME` does not help: the JVM reads the passwd database, not the
#      environment.
#
#   2. `java.io.IOException: Invalid UID, could not determine effective user`, caused by
#      `LoginException: NullPointerException: invalid null input: name` -- Hadoop's
#      `UserGroupInformation` logs in via JAAS during `SparkContext` construction and gets a null
#      username from the same failed lookup. Spark 4.x still initialises Hadoop's security stack
#      in `local` mode with no Hadoop in sight.
#
# A passwd entry fixes both causes at the source rather than patching each symptom.
#
# `--network` puts the driver on the same bridge as the two nodes, which is what makes the
# addresses Cassandra advertises in `system.local.rpc_address` routable from it. See
# README-ENVIRONMENT.md, "Networking".
START="$(date +%s)"
set +e
docker run --rm \
    --name cdm-bench-spark \
    --network "${BENCH_NETWORK}" \
    --user "$(id -u):$(id -g)" \
    --env HOME=/work/out \
    --volume "${PASSWD}:/etc/passwd:ro" \
    --volume "${OUTDIR}:/work/out" \
    --workdir /work/out \
    "${BENCH_IMAGE}" \
    spark-submit \
    --driver-java-options "-Duser.home=/work/out" \
    --master "local[${LOCAL_THREADS}]" \
    --driver-memory "${DRIVER_MEMORY}" \
    --conf "spark.ui.enabled=${SPARK_UI}" \
    --conf "spark.local.dir=/work/out/spark-local" \
    --conf "spark.cdm.schema.origin.keyspaceTable=${KEYSPACE_TABLE}" \
    --properties-file "/work/out/$(basename "${PROPS}")" \
    --class "${CDM_CLASS}" \
    "/opt/cdm/${CDM_JAR}" \
    > "${LOG}" 2>&1
STATUS=$?
set -e
ELAPSED=$(($(date +%s) - START))

# CDM prints its result as a block of counter lines. Surfacing them here means the operator does
# not have to know that a 4 MB Spark log has twelve interesting lines in it.
printf '\n[submit] exit %d after %ds\n' "${STATUS}" "${ELAPSED}"
if grep -qE 'Read Record Count|Write Record Count|Error Record Count' "${LOG}"; then
    printf '[submit] CDM counters:\n'
    grep -E 'Record Count|Skipped Record Count|Valid Record Count' "${LOG}" | sed 's/^/  /'
else
    printf '[submit] no CDM counter block in the log; last 30 lines:\n'
    tail -30 "${LOG}" | sed 's/^/  /'
fi

# Shuffle scratch is regenerable and can be hundreds of MiB; the log and the properties are the
# evidence and stay.
rm -rf "${OUTDIR}/spark-local"

# CDM's exit status does not mean what a caller assumes it means.
#
# Observed: a run that reported `Final Error Record Count: 34454` and left 1,465 rows missing on
# the target still exited 0, with `spark.cdm.perfops.errorLimit` set to 0. `docs/MIGRATION_FROM_JAVA.md`
# item 42 records the same shape for `DiffData` -- nothing in `src/main/scala` calls `System.exit`,
# so spark-submit succeeds unless the job throws. A benchmark harness that trusts the exit status
# will therefore happily time a run that dropped a sixth of the data, and report it as a win.
#
# So the counter block is the authority, and a non-zero error count fails here even though CDM was
# content. This is deliberately *stricter* than Java CDM, which is the safe direction: it can only
# reject a run, never accept a bad one.
ERRORS="$(grep -oE 'Final Error Record Count: [0-9]+' "${LOG}" | tail -1 | grep -oE '[0-9]+$' || true)"
if [ -n "${ERRORS}" ] && [ "${ERRORS}" -ne 0 ]; then
    printf '[submit] FAILED: %s error records; CDM exited %d anyway. Not a usable measurement.\n' \
        "${ERRORS}" "${STATUS}" >&2
    exit 1
fi

exit "${STATUS}"
