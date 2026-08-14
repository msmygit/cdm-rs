#!/usr/bin/env bash
#
# Tier 3 of docs/BENCHMARKS.md: run Java CDM and cdm-rs over the same dataset, on the same
# machine, back to back, and report both throughputs (`NFR-004`, `TST-060`).
#
# Usage:
#   bench/java-comparison/run.sh                             # the reference workload
#   bench/java-comparison/run.sh --workload all --repeats 2  # everything, both orders
#   bench/java-comparison/run.sh --rows 250000               # a different dataset size
#   bench/java-comparison/run.sh --skip-java                 # cdm-rs only; no Spark needed
#
# Exit status is 0 whenever the harness completed, whatever the numbers were, and non-zero only
# when the harness itself could not do its job. A throughput ratio never fails this script.
#
# ------------------------------------------------------------------------------------------------
# Why a shell script and not `cargo xtask java-comparison`
# ------------------------------------------------------------------------------------------------
#
# Everything this runner does is process orchestration against things that are not Rust: `docker`,
# `spark-submit`, `cqlsh`, `nb5`, and two implementations invoked as binaries. An xtask would wrap
# each of those in `std::process::Command` and gain nothing, while costing three things:
#
#   1. it puts a workspace build on the critical path of a benchmark whose whole point is to time a
#      *released* binary — and it would have to compile itself before it could tell you Spark is
#      missing;
#   2. the other two thirds of this harness (`environment/`, `workloads/`) are shell, so an xtask
#      would split one procedure across two languages and two error models;
#   3. `cargo xtask bench` (tier 2) is an xtask precisely because it drives the *library*, in
#      process, and can therefore assert on typed results. This runner drives no library at all —
#      it starts containers and times other people's processes.
#
# The rule the repository already follows: in-process measurement is an xtask, external process
# orchestration is a script. `scripts/reclaim-agent-space.sh` is the house style for the second.
#
# ------------------------------------------------------------------------------------------------
# Reporting stance
# ------------------------------------------------------------------------------------------------
#
# This script reports what happened. There are no retries, no "best of N", and no fallback that
# gives one implementation a second attempt the other did not get. `--repeats` takes a median and
# applies it identically to both sides. If the Java side fails, the cdm-rs number is still
# recorded and **no ratio is emitted at all** — a ratio against a failed or partial Java run is
# worse than no number, because it looks like a result.
#
# ------------------------------------------------------------------------------------------------
# The three seams
# ------------------------------------------------------------------------------------------------
#
# Two sibling directories are owned elsewhere: `environment/` (Spark, the pinned Java CDM jar, the
# containers) and `workloads/` (the datasets and the config-equivalence audit). This runner talks
# to them through exactly three seams, each of which prefers the sibling's script and falls back to
# something self-contained so that a cdm-rs-only run works in a checkout that has neither:
#
#   harness_clusters_up / harness_clusters_down
#       Preferred: environment/clusters-up.sh, environment/clusters-down.sh, with addresses,
#       image and heap read from environment/versions.env.
#       Fallback: two `cassandra:5.0` containers started here, published on loopback.
#
#   harness_seed <workload> <rows>
#       Preferred: workloads/seed.sh <workload> <rows> <origin-host:port> <target-host:port>,
#       which must accept a row count of 0 — schema on both sides, no rows.
#       Fallback: nosqlbench, driving workloads/<workload>.nb5.yaml's `default.schema` and
#       `default.load` scenarios, which is how those files document themselves.
#
#   harness_run_java <properties> <log>
#       Preferred: environment/run-java-cdm.sh <properties-file>, which gives both implementations
#       the same file, byte for byte.
#       Fallback: environment/submit-migrate.sh <keyspace.table> <outdir>, which builds its own
#       properties from a template; the settings that decide how much work is done are then mapped
#       across from the workload file into the environment variables that script documents, and
#       the result records `properties_equivalence: "mapped"` so that nobody reads it as byte
#       identity. See docs/BENCHMARKS.md §5.
#
# If the sibling interfaces settle differently, those five functions are the only thing that
# changes. Everything else in this file is measurement and bookkeeping.

set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "${SCRIPT_DIR}/../.." && pwd)"
WORKLOAD_DIR="${SCRIPT_DIR}/workloads"
ENVIRONMENT_DIR="${SCRIPT_DIR}/environment"

# --- defaults -------------------------------------------------------------------------------------

# `narrow` alone by default: it is the workload `NFR-004` is about and the one a scheduled run must
# always produce. The other two are opt-in, because three workloads is three times the runtime and
# a scheduled job has a six-hour ceiling.
WORKLOADS_REQUESTED="narrow"
ROWS=100000
REPEATS=1
FIRST="auto"
OUT_DIR=""
CDM_BIN=""
SKIP_JAVA=0
SKIP_PROBE=0
KEEP_CONTAINERS=0
IMAGE_OVERRIDE=""

# Fallback container settings, used only when `environment/` is absent. They mirror
# `crates/cdm-testkit/src/containers.rs` so that a fallback run is comparable with tier 2.
CASSANDRA_IMAGE="cassandra:5.0"
CASSANDRA_HEAP_MIB=1024
CASSANDRA_NEWSIZE_MIB=256
ORIGIN_CONTAINER="cdm-bench-origin"
TARGET_CONTAINER="cdm-bench-target"
ORIGIN_HOST="127.0.0.1"
TARGET_HOST="127.0.0.1"
ORIGIN_PORT="9042"
TARGET_PORT="9043"
NATIVE_PORT="9042"

# Where `cqlsh` connects *from inside the node's own container*, which is not the same address the
# migrator connects to. In the fallback configuration the target is published on host port 9043 but
# still listens on 9042 inside its container, and a probe that used the host port would time out
# against a node that is perfectly healthy.
ORIGIN_CQLSH_HOST="127.0.0.1"
ORIGIN_CQLSH_PORT="9042"
TARGET_CQLSH_HOST="127.0.0.1"
TARGET_CQLSH_PORT="9042"

# A cold `cassandra:5.0` on a shared runner is routinely 45–90 seconds to first CQL. Reaching this
# ceiling means the node is not coming up at all, not that it is slow.
NODE_READY_TIMEOUT=420

# --- argument parsing -------------------------------------------------------------------------------

usage() {
    sed -n '3,13p' "$0" | sed 's/^# \{0,1\}//'
    exit "${1:-0}"
}

while [ $# -gt 0 ]; do
    case "$1" in
        --workload)           shift; WORKLOADS_REQUESTED="${1:?--workload needs a value}" ;;
        --rows)               shift; ROWS="${1:?--rows needs a value}" ;;
        --repeats)            shift; REPEATS="${1:?--repeats needs a value}" ;;
        --first)              shift; FIRST="${1:?--first needs a value}" ;;
        --out)                shift; OUT_DIR="${1:?--out needs a value}" ;;
        --cdm-bin)            shift; CDM_BIN="${1:?--cdm-bin needs a value}" ;;
        --image)              shift; IMAGE_OVERRIDE="${1:?--image needs a value}" ;;
        --skip-java)          SKIP_JAVA=1 ;;
        --skip-startup-probe) SKIP_PROBE=1 ;;
        --keep-containers)    KEEP_CONTAINERS=1 ;;
        -h|--help)            usage 0 ;;
        *)                    printf 'unknown argument: %s\n\n' "$1" >&2; usage 2 ;;
    esac
    shift
done

case "$FIRST" in
    auto|cdm-rs|java-cdm) ;;
    *) printf -- '--first must be cdm-rs, java-cdm or auto (got %s)\n' "$FIRST" >&2; exit 2 ;;
esac

[ "$WORKLOADS_REQUESTED" = "all" ] && WORKLOADS_REQUESTED="narrow,wide,collections"
IFS=',' read -r -a WORKLOADS <<< "$WORKLOADS_REQUESTED"

[ -n "$OUT_DIR" ] || OUT_DIR="${SCRIPT_DIR}/results/$(date -u +%Y%m%dT%H%M%SZ)"
mkdir -p "${OUT_DIR}/runs" "${OUT_DIR}/logs" "${OUT_DIR}/conf"

# --- small helpers ----------------------------------------------------------------------------------

log()  { printf '[java-comparison] %s\n' "$*" >&2; }
warn() { printf '[java-comparison] WARNING: %s\n' "$*" >&2; }
die()  { printf '[java-comparison] FATAL: %s\n' "$*" >&2; exit 1; }

# Seconds as a decimal. `EPOCHREALTIME` is microsecond-resolution and needs no subprocess, which
# matters because it is read immediately either side of the thing being measured. It is bash 5
# only; `date +%s` is the fallback, and a whole-second clock is still adequate against runs that
# last tens of seconds.
now_seconds() {
    if [ -n "${EPOCHREALTIME:-}" ]; then
        printf '%s' "${EPOCHREALTIME/,/.}"
    else
        date +%s
    fi
}

# `awk` rather than bash arithmetic throughout: every figure here is a fraction of a second or a
# rate, and bash integer arithmetic can express neither.
fnum() { awk -v a="$1" 'BEGIN { printf "%.6f", a }'; }
fsub() { awk -v a="$1" -v b="$2" 'BEGIN { printf "%.6f", a - b }'; }

# A duration as a JSON number, or the literal `null` when it was never measured. Every optional
# figure goes through this: an absent measurement must not become a zero, because a zero is a
# claim and an absence is not.
num_or_null() { if [ -n "${1:-}" ]; then fnum "$1"; else printf 'null'; fi; }

# rows / seconds, or `null` when the divisor is missing or not positive.
frate() {
    awk -v rows="$1" -v secs="$2" 'BEGIN {
        if (secs == "" || secs + 0 <= 0 || rows + 0 <= 0) { print "null" }
        else { printf "%.2f", rows / secs }
    }'
}

sha256_of() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | cut -d' ' -f1
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$1" | cut -d' ' -f1
    else
        printf 'unavailable'
    fi
}

# One value out of a Java properties file. Java CDM's own files use whitespace as the separator and
# `cdm.properties` uses `=`, so both are accepted; the first occurrence wins, as in Java.
props_get() {
    sed -n "s/^[[:space:]]*$(printf '%s' "$2" | sed 's/\./\\./g')[[:space:]]*[=:[:space:]][[:space:]]*//p" \
        "$1" 2>/dev/null | head -n1 | tr -d '\r' | sed 's/[[:space:]]*$//'
}

# --- preflight ---------------------------------------------------------------------------------------

command -v docker >/dev/null 2>&1 || die "docker is required: this harness starts its own nodes."
docker info >/dev/null 2>&1 || die "docker is installed but not answering; is the daemon running?"
command -v jq >/dev/null 2>&1 || die "jq is required to emit the machine-readable results."

# `environment/versions.env` is the single place the pinned Spark, jar, image, heap, network and
# node addresses are declared. Sourcing it rather than restating any of them is what keeps this
# runner from becoming a second, divergent copy of the environment's decisions.
ENVIRONMENT_PINNED=0
if [ -r "${ENVIRONMENT_DIR}/versions.env" ]; then
    # shellcheck source=/dev/null
    . "${ENVIRONMENT_DIR}/versions.env"
    ENVIRONMENT_PINNED=1
    ORIGIN_HOST="${ORIGIN_IP:-$ORIGIN_HOST}"
    TARGET_HOST="${TARGET_IP:-$TARGET_HOST}"
    ORIGIN_PORT="${NATIVE_PORT:-9042}"
    TARGET_PORT="${NATIVE_PORT:-9042}"
    # Pinned addresses are routable from inside the containers too, and using them proves the
    # address each node advertises in `system.local.rpc_address` is the one clients can reach.
    ORIGIN_CQLSH_HOST="$ORIGIN_HOST"; ORIGIN_CQLSH_PORT="$ORIGIN_PORT"
    TARGET_CQLSH_HOST="$TARGET_HOST"; TARGET_CQLSH_PORT="$TARGET_PORT"
fi
# The flag wins over the pinning, because `--image` exists precisely to answer "does the ratio hold
# on 4.1?" — but it is recorded in the fingerprint, so a run on a non-default image says so.
[ -n "$IMAGE_OVERRIDE" ] && CASSANDRA_IMAGE="$IMAGE_OVERRIDE"

# The binary under test: the release profile, because the number published has to be the one a user
# gets, not the one a debug build gives.
if [ -z "$CDM_BIN" ]; then
    if [ -x "${REPO_ROOT}/target/dist/cdm" ]; then
        CDM_BIN="${REPO_ROOT}/target/dist/cdm"
    elif [ -x "${REPO_ROOT}/target/release/cdm" ]; then
        CDM_BIN="${REPO_ROOT}/target/release/cdm"
    else
        log "building cdm (profile dist) — no binary found under target/"
        (cd "$REPO_ROOT" && cargo build --profile dist --bin cdm >&2)
        CDM_BIN="${REPO_ROOT}/target/dist/cdm"
    fi
fi
[ -x "$CDM_BIN" ] || die "cdm binary not executable: ${CDM_BIN}"
CDM_VERSION="$("$CDM_BIN" --version 2>/dev/null | head -n1 || printf 'unknown')"

# Which Java seam is available, if any. The Java side is optional *to the harness* and mandatory
# *to the claim*: when it cannot run, every cdm-rs figure is still collected and every comparison
# says, in the file, why the other half is missing.
JAVA_SEAM="none"
JAVA_STATUS="ok"
JAVA_UNAVAILABLE_REASON=""
if [ "$SKIP_JAVA" -eq 1 ]; then
    JAVA_STATUS="skipped"
    JAVA_UNAVAILABLE_REASON="--skip-java was passed"
elif [ -x "${ENVIRONMENT_DIR}/run-java-cdm.sh" ]; then
    JAVA_SEAM="run-java-cdm"
elif [ -x "${ENVIRONMENT_DIR}/submit-migrate.sh" ]; then
    JAVA_SEAM="submit-migrate"
else
    JAVA_STATUS="unavailable"
    JAVA_UNAVAILABLE_REASON="neither environment/run-java-cdm.sh nor environment/submit-migrate.sh is executable"
fi

# `environment/setup.sh` prepares Spark and the pinned jar. Sourced, not executed: the contract is
# that it *sets* JAVA_CDM_JAR and SPARK_HOME, and a child process cannot set them in this one.
if [ "$JAVA_STATUS" = "ok" ] && [ -r "${ENVIRONMENT_DIR}/setup.sh" ]; then
    if [ -z "${JAVA_CDM_JAR:-}" ] || [ -z "${SPARK_HOME:-}" ]; then
        log "sourcing environment/setup.sh"
        # shellcheck source=/dev/null
        . "${ENVIRONMENT_DIR}/setup.sh" || warn "environment/setup.sh failed; continuing to see whether the seam still runs"
    fi
fi
[ "$JAVA_STATUS" = "ok" ] || warn "the Java side will not run: ${JAVA_UNAVAILABLE_REASON}"

# --- the environment fingerprint ------------------------------------------------------------------
#
# A throughput figure without its hardware is not a result (docs/BENCHMARKS.md §5). Every run
# document carries this block, so a JSON file that has been detached from its directory is still
# interpretable.

host_cpus()            { nproc 2>/dev/null || sysctl -n hw.ncpu 2>/dev/null || printf '0'; }
host_disk_free_bytes() { df -k . | awk 'NR==2 { printf "%d", $4 * 1024 }'; }

host_mem_bytes() {
    if [ -r /proc/meminfo ]; then
        awk '/^MemTotal:/ { printf "%d", $2 * 1024 }' /proc/meminfo
    else
        sysctl -n hw.memsize 2>/dev/null || printf '0'
    fi
}

host_cpu_model() {
    if [ -r /proc/cpuinfo ]; then
        awk -F': ' '/^model name/ { print $2; exit }' /proc/cpuinfo
    else
        sysctl -n machdep.cpu.brand_string 2>/dev/null || printf 'unknown'
    fi
}

java_version_string() {
    command -v java >/dev/null 2>&1 || { printf 'in-image or absent'; return; }
    java -version 2>&1 | head -n1
}

spark_version_string() {
    if [ -n "${SPARK_VERSION:-}" ]; then printf 'spark %s' "$SPARK_VERSION"
    elif [ -n "${SPARK_HOME:-}" ] && [ -r "${SPARK_HOME}/RELEASE" ]; then head -n1 "${SPARK_HOME}/RELEASE"
    else printf 'unknown'
    fi
}

java_cdm_identity() {
    if [ -n "${CDM_JAR:-}" ]; then printf '%s' "$CDM_JAR"
    elif [ -n "${JAVA_CDM_JAR:-}" ]; then basename "$JAVA_CDM_JAR"
    else printf 'unknown'
    fi
}

version_for() {
    if [ "$1" = "cdm-rs" ]; then printf '%s' "$CDM_VERSION"; else java_cdm_identity; fi
}

ENV_JSON="${OUT_DIR}/environment.json"
jq -n \
    --arg os "$(uname -s)" \
    --arg kernel "$(uname -r)" \
    --arg arch "$(uname -m)" \
    --arg cpu_model "$(host_cpu_model)" \
    --argjson cpus "$(host_cpus)" \
    --argjson memory_bytes "$(host_mem_bytes)" \
    --argjson disk_free_bytes "$(host_disk_free_bytes)" \
    --arg docker "$(docker --version 2>/dev/null || printf 'unknown')" \
    --arg cassandra_image "$CASSANDRA_IMAGE" \
    --arg cassandra_heap "${CASSANDRA_HEAP_MIB}M" \
    --arg java "$(java_version_string)" \
    --arg spark "$(spark_version_string)" \
    --arg java_cdm "$(java_cdm_identity)" \
    --arg java_seam "$JAVA_SEAM" \
    --argjson environment_pinned "$ENVIRONMENT_PINNED" \
    --arg cdm_rs "$CDM_VERSION" \
    --arg cdm_rs_commit "$(git -C "$REPO_ROOT" rev-parse HEAD 2>/dev/null || printf 'unknown')" \
    --arg ci_run "${GITHUB_RUN_ID:-local}" \
    '{os: $os, kernel: $kernel, arch: $arch, cpu_model: $cpu_model, cpus: $cpus,
      memory_bytes: $memory_bytes, disk_free_bytes: $disk_free_bytes, docker: $docker,
      cassandra_image: $cassandra_image, cassandra_heap: $cassandra_heap,
      java: $java, spark: $spark, java_cdm: $java_cdm, java_seam: $java_seam,
      environment_pinned: ($environment_pinned == 1),
      cdm_rs_version: $cdm_rs, cdm_rs_commit: $cdm_rs_commit, ci_run: $ci_run}' > "$ENV_JSON"

log "environment recorded in ${ENV_JSON}"

# --- seam 1: the clusters ----------------------------------------------------------------------------
#
# REQUIREMENT: fresh containers per implementation.
#
# Reusing one origin/target pair across both implementations is the easiest way to fabricate a
# large difference. Whichever ran second would read origin out of a warm page cache and write into
# a target holding the first run's SSTables and its compaction backlog. Neither effect is small and
# both favour whichever side happens to go second. So: nodes destroyed and recreated, schema
# recreated, data reseeded, for every single measured run — which is why `harness_clusters_up`
# always tears down first rather than trusting an idempotent "already running".

cql_origin() { docker exec -i "$ORIGIN_CONTAINER" cqlsh "$ORIGIN_CQLSH_HOST" "$ORIGIN_CQLSH_PORT" "$@"; }
cql_target() { docker exec -i "$TARGET_CONTAINER" cqlsh "$TARGET_CQLSH_HOST" "$TARGET_CQLSH_PORT" "$@"; }

harness_clusters_down() {
    if [ -x "${ENVIRONMENT_DIR}/clusters-down.sh" ]; then
        "${ENVIRONMENT_DIR}/clusters-down.sh" >/dev/null 2>&1 || true
        return 0
    fi
    docker rm -f "$ORIGIN_CONTAINER" "$TARGET_CONTAINER" >/dev/null 2>&1 || true
}

harness_clusters_up() {
    if [ -x "${ENVIRONMENT_DIR}/clusters-up.sh" ]; then
        "${ENVIRONMENT_DIR}/clusters-up.sh" >&2 || return 1
        return 0
    fi
    fallback_start_node "$ORIGIN_CONTAINER" "$ORIGIN_PORT" cdm-bench-origin || return 1
    fallback_start_node "$TARGET_CONTAINER" "$TARGET_PORT" cdm-bench-target || return 1
    fallback_wait_for_cql "$ORIGIN_CONTAINER" || return 1
    fallback_wait_for_cql "$TARGET_CONTAINER" || return 1
}

# The self-contained pair, for a checkout with no `environment/` — a developer running
# `--skip-java` to get the cdm-rs half. Published on loopback because there is no shared bridge
# network to put a Spark driver on in this configuration, and no Spark driver either.
fallback_start_node() {
    local name="$1" host_port="$2" cluster="$3"
    docker rm -f "$name" >/dev/null 2>&1 || true
    # MAX_HEAP_SIZE and HEAP_NEWSIZE are set as a pair: `cassandra-env.sh` aborts when only one is
    # set under CMS. Uncapped, each node sizes its heap from host RAM and pre-touches it — roughly
    # 4 GiB per node on a 16 GiB box, which does not leave room for two nodes and a Spark JVM.
    docker run --detach --name "$name" \
        --publish "127.0.0.1:${host_port}:9042" \
        --env "CASSANDRA_CLUSTER_NAME=${cluster}" \
        --env "MAX_HEAP_SIZE=${CASSANDRA_HEAP_MIB}M" \
        --env "HEAP_NEWSIZE=${CASSANDRA_NEWSIZE_MIB}M" \
        "$CASSANDRA_IMAGE" >/dev/null
}

# Readiness is proved by a query, not by a log line: "Startup complete" is written before the
# native transport binds, so trusting it hands the next step a node that refuses the connection.
fallback_wait_for_cql() {
    local name="$1" deadline=$((SECONDS + NODE_READY_TIMEOUT))
    while [ "$SECONDS" -lt "$deadline" ]; do
        if docker exec "$name" cqlsh -e 'SELECT release_version FROM system.local' >/dev/null 2>&1; then
            return 0
        fi
        if ! docker ps --format '{{.Names}}' | grep -qx "$name"; then
            warn "${name} exited during startup; last 20 log lines follow"
            docker logs --tail 20 "$name" >&2 || true
            return 1
        fi
        sleep 3
    done
    warn "${name} did not answer CQL within ${NODE_READY_TIMEOUT}s"
    docker logs --tail 20 "$name" >&2 || true
    return 1
}

# `SELECT COUNT(*)` from outside both implementations. This is the check that distinguishes
# "migrated N rows quickly" from "counted to N quickly", and it is the same check
# `crates/cdm-testkit/src/macrobench.rs` makes before it will report a tier-2 figure at all.
count_rows() {
    local side="$1" table="$2" out
    if [ "$side" = "origin" ]; then
        out="$(cql_origin -e "SELECT COUNT(*) FROM ${table};" 2>/dev/null)" || { printf 'null'; return 0; }
    else
        out="$(cql_target -e "SELECT COUNT(*) FROM ${table};" 2>/dev/null)" || { printf 'null'; return 0; }
    fi
    # cqlsh prints a header, a rule, the value, a blank line and a row count. The value is the only
    # bare integer alone on a line.
    printf '%s' "$out" | awk '/^ *[0-9]+ *$/ { print $1; found = 1; exit } END { if (!found) print "null" }'
}

# --- seam 2: the dataset ------------------------------------------------------------------------------

harness_properties_for() { printf '%s/%s.properties' "$WORKLOAD_DIR" "$1"; }

# The workload's `keyspace.table`, for the independent count and for the seams that take a table
# rather than a properties file.
harness_table_for() {
    local workload="$1" side="${2:-origin}" file="${WORKLOAD_DIR}/$1.table"
    if [ -r "$file" ]; then
        head -n1 "$file" | tr -d '[:space:]'
        return 0
    fi
    props_get "$(harness_properties_for "$workload")" "spark.cdm.schema.${side}.keyspaceTable"
}

# Creates the workload's schema on BOTH sides and seeds exactly <rows> rows into origin. A row
# count of 0 is legitimate and means "schema only" — that is what the startup probe runs against.
harness_seed() {
    local workload="$1" rows="$2"

    if [ -x "${WORKLOAD_DIR}/seed.sh" ]; then
        "${WORKLOAD_DIR}/seed.sh" "$workload" "$rows" \
            "${ORIGIN_HOST}:${ORIGIN_PORT}" "${TARGET_HOST}:${TARGET_PORT}" >&2
        return $?
    fi

    # nosqlbench, driving the scenarios the `.nb5.yaml` files document themselves with:
    #   nb5 <w>.nb5 default.schema keyspace=origin host=<origin>
    #   nb5 <w>.nb5 default.load   keyspace=origin host=<origin> load-cycles=<rows>
    local yaml="${WORKLOAD_DIR}/${workload}.nb5.yaml"
    [ -r "$yaml" ] || { warn "no seeding seam: neither workloads/seed.sh nor ${yaml}"; return 1; }
    nb5_available || { warn "nosqlbench (nb5) is needed to load ${yaml} and is not on PATH"; return 1; }

    local origin_ks target_ks
    origin_ks="$(harness_table_for "$workload" origin)"; origin_ks="${origin_ks%%.*}"
    target_ks="$(harness_table_for "$workload" target)"; target_ks="${target_ks%%.*}"

    nb5_run "$yaml" default.schema "keyspace=${origin_ks}" "host=${ORIGIN_HOST}" "port=${ORIGIN_PORT}" || return 1
    nb5_run "$yaml" default.schema "keyspace=${target_ks}" "host=${TARGET_HOST}" "port=${TARGET_PORT}" || return 1
    [ "$rows" -eq 0 ] && return 0
    nb5_run "$yaml" default.load "keyspace=${origin_ks}" "host=${ORIGIN_HOST}" "port=${ORIGIN_PORT}" \
        "load-cycles=${rows}" || return 1
}

nb5_available() {
    command -v nb5 >/dev/null 2>&1 || { [ -n "${NB5_JAR:-}" ] && [ -r "${NB5_JAR}" ]; }
}

nb5_run() {
    local yaml="$1"; shift
    if command -v nb5 >/dev/null 2>&1; then
        nb5 "$yaml" "$@" --progress console:30s >&2
    else
        java -jar "$NB5_JAR" "$yaml" "$@" --progress console:30s >&2
    fi
}

# Copies the workload's properties and appends the connection details for the nodes this run just
# started. Nothing else is touched: every tuning knob stays exactly as the workload author wrote
# it, and the same file is handed to both implementations. Its SHA-256 goes into the result, so the
# config-equivalence audit can confirm both sides were given identical bytes.
materialise_properties() {
    local workload="$1" dest="$2" src
    src="$(harness_properties_for "$workload")"
    [ -r "$src" ] || { warn "no properties file at ${src}"; return 1; }

    grep -v -E '^[[:space:]]*spark\.cdm\.connect\.(origin|target)\.(host|port)([[:space:]]|=|:)' \
        "$src" > "$dest" || true
    cat >> "$dest" <<EOF

# Appended by bench/java-comparison/run.sh: the nodes this run started. Everything above is the
# workload exactly as committed, and this file is given to both implementations byte for byte.
spark.cdm.connect.origin.host=${ORIGIN_HOST}
spark.cdm.connect.origin.port=${ORIGIN_PORT}
spark.cdm.connect.target.host=${TARGET_HOST}
spark.cdm.connect.target.port=${TARGET_PORT}
EOF
}

# --- seam 3: running an implementation -----------------------------------------------------------------

# How the Java side was configured, for the result document. `file` means both implementations read
# the same bytes; `mapped` means the environment built its own properties from a template and the
# workload's settings were carried across as the environment variables that script documents.
JAVA_PROPERTIES_EQUIVALENCE="not run"

harness_run_java() {
    local properties="$1" log_file="$2" workload="$3"

    if [ "$JAVA_SEAM" = "run-java-cdm" ]; then
        JAVA_PROPERTIES_EQUIVALENCE="file"
        "${ENVIRONMENT_DIR}/run-java-cdm.sh" "$properties" > "$log_file" 2>&1
        return $?
    fi

    # `submit-migrate.sh <keyspace.table> [outdir]` generates its own properties from
    # `cdm.properties.template` and takes the settings that decide how much work is done from the
    # environment. Carrying them across from the workload file is what keeps the two sides
    # configured alike; it is not byte identity, and the result says so.
    JAVA_PROPERTIES_EQUIVALENCE="mapped"
    local table
    table="$(harness_table_for "$workload" origin)"
    CDM_NUM_PARTS="$(props_get "$properties" spark.cdm.perfops.numParts)" \
    CDM_BATCH_SIZE="$(props_get "$properties" spark.cdm.perfops.batchSize)" \
    CDM_FETCH_SIZE="$(props_get "$properties" spark.cdm.perfops.fetchSizeInRows)" \
    CDM_RATELIMIT="$(props_get "$properties" spark.cdm.perfops.ratelimit.origin)" \
        "${ENVIRONMENT_DIR}/submit-migrate.sh" "$table" "${OUT_DIR}/logs/java" > "$log_file" 2>&1
    return $?
}

# Invokes one implementation once. Sets RUN_STATUS, RUN_SECONDS and RUN_ROWS_WRITTEN.
invoke_implementation() {
    local impl="$1" properties="$2" log_file="$3" summary_file="$4" workload="$5"
    local started finished status=0

    # Reset every global this function reports through. These outlive one call — the suite loops
    # over workloads, implementations and repetitions — so a value left behind by an earlier run
    # would be attributed to this one.
    RUN_STATUS="ok"
    RUN_SECONDS=""
    RUN_ROWS_WRITTEN="null"
    RUN_ERROR_RECORDS="null"

    started="$(now_seconds)"
    case "$impl" in
        cdm-rs)
            "$CDM_BIN" migrate --properties-file "$properties" --summary-out "$summary_file" \
                > "$log_file" 2>&1 || status=$?
            ;;
        java-cdm)
            harness_run_java "$properties" "$log_file" "$workload" || status=$?
            ;;
        *) die "unknown implementation ${impl}" ;;
    esac
    finished="$(now_seconds)"

    RUN_SECONDS="$(fsub "$finished" "$started")"
    if [ "$status" -ne 0 ]; then
        RUN_STATUS="failed(exit ${status})"
        return 0
    fi

    # What the job says it wrote. For cdm-rs that is the `MET-033` summary's committed WRITE
    # counter; for Java CDM it is the `Final Write Record Count` line of the counter block — which
    # cdm-rs reproduces character for character (`MET-006`, `COMPAT-004`), so one regex serves as
    # the fallback for both.
    if [ "$impl" = "cdm-rs" ] && [ -r "$summary_file" ]; then
        RUN_ROWS_WRITTEN="$(jq -r '.counters.WRITE // "null"' "$summary_file" 2>/dev/null || printf 'null')"
    fi
    if [ "$RUN_ROWS_WRITTEN" = "null" ] || [ -z "$RUN_ROWS_WRITTEN" ]; then
        RUN_ROWS_WRITTEN="$(grep -oE '(Final )?Write Record Count: *[0-9]+' "$log_file" \
            | tail -n1 | grep -oE '[0-9]+$' || true)"
        [ -n "$RUN_ROWS_WRITTEN" ] || RUN_ROWS_WRITTEN="null"
    fi

    # The error counter, read for its diagnostic value rather than as the safety net.
    #
    # Java CDM exits 0 after losing data: a run that reported 34,454 error records and a short
    # target still returned success with `errorLimit=0` (the shape `MIGRATION_FROM_JAVA.md` item 42
    # records for `DiffData`). A harness that trusted `$?` would score lost rows as a faster run.
    #
    # It is not what protects the result — the independent `SELECT COUNT(*)` below does that, and
    # catches loss whether or not either implementation admits to it. But "Java reported 135264
    # error records" is a diagnosis, whereas "the counts disagree" is only a symptom, and the run
    # that produces it costs hours to repeat.
    RUN_ERROR_RECORDS="$(grep -oE '(Final )?Error Record Count: *[0-9]+' "$log_file" \
        | tail -n1 | grep -oE '[0-9]+$' || true)"
    [ -n "$RUN_ERROR_RECORDS" ] || RUN_ERROR_RECORDS="null"
    return 0
}

# The startup floor, measured rather than guessed.
#
# REQUIREMENT: report cold and steady-state separately. Cold includes Spark's JVM and executor
# startup; steady-state excludes it. Java's startup is tens of seconds, and it must be neither
# hidden nor allowed to dominate.
#
# The measurement is the same procedure on both sides: migrate the workload's table with zero rows
# in it. That is a complete invocation — process spawn, JVM, SparkSession, executors, schema
# introspection, statement preparation, the token plan, an empty scan of every range, the counter
# block, exit — minus only the row work. Subtracting it from the measured run leaves the time
# attributable to moving rows.
#
# It is a slight OVER-estimate of startup, because scanning the ranges of an empty table is not
# free. That direction is deliberate: over-estimating startup shortens the steady-state window and
# therefore *raises* the reported steady-state rate — by more for the implementation with the
# larger startup, which is Java. The bias works against cdm-rs's ratio, never for it. It is not a
# log-parsing exercise for the same reason: a marker grepped out of Spark's log4j output would
# depend on a logging configuration neither implementation guarantees.
measure_startup() {
    local impl="$1" properties="$2" log_file="$3" workload="$4"
    invoke_implementation "$impl" "$properties" "$log_file" "${log_file}.summary.json" "$workload"
    printf '%s' "$RUN_SECONDS"
}

# --- one measured run -----------------------------------------------------------------------------------

run_one() {
    local impl="$1" workload="$2" repeat="$3" order_index="$4"
    local tag="${workload}-${impl}-r${repeat}"
    local run_json="${OUT_DIR}/runs/${tag}.json"
    local properties="${OUT_DIR}/conf/${tag}.properties"
    local migrate_log="${OUT_DIR}/logs/${tag}.migrate.log"
    local probe_log="${OUT_DIR}/logs/${tag}.startup-probe.log"
    local validate_log="${OUT_DIR}/logs/${tag}.validate.log"
    local summary_file="${OUT_DIR}/runs/${tag}.summary.json"
    local validate_summary="${OUT_DIR}/runs/${tag}.validate.json"

    local status="ok" note="" table="" seeded=""
    local startup="" steady="" cold="" rows_written="null" target_rows="null"
    local validate_discrepancies="null" validate_status="not run"
    local started_at finished_at

    JAVA_PROPERTIES_EQUIVALENCE="not run"
    started_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    log "=== ${impl} / ${workload} / repeat ${repeat} / order ${order_index} ==="

    # 1. Fresh nodes. Torn down first, unconditionally: see the note on seam 1.
    harness_clusters_down
    if ! harness_clusters_up; then
        status="failed"
        note="the origin/target pair did not come up"
    fi

    if [ "$status" = "ok" ]; then
        table="$(harness_table_for "$workload" origin || true)"
        [ -n "$table" ] || { status="failed"; note="cannot determine the workload's keyspace.table"; }
    fi

    if [ "$status" = "ok" ]; then
        materialise_properties "$workload" "$properties" || {
            status="failed"; note="the workload's properties file could not be materialised"
        }
    fi

    # 2. Schema on both sides with no rows in it, and the startup probe against that.
    if [ "$status" = "ok" ] && [ "$SKIP_PROBE" -eq 0 ]; then
        if harness_seed "$workload" 0; then
            startup="$(measure_startup "$impl" "$properties" "$probe_log" "$workload")"
            if [ "$RUN_STATUS" != "ok" ]; then
                warn "the startup probe for ${impl} did not succeed (${RUN_STATUS}); steady state will be null"
                startup=""
            fi
        else
            warn "could not create the empty schema; steady state will be null"
        fi
    fi

    # 3. Seed origin, and confirm the dataset is the size it claims to be. A run over a short
    #    dataset is a different workload, not a fast one.
    if [ "$status" = "ok" ]; then
        if ! harness_seed "$workload" "$ROWS"; then
            status="failed"; note="seeding failed"
        else
            seeded="$(count_rows origin "$table")"
            if [ "$seeded" != "$ROWS" ]; then
                status="failed"
                note="origin holds ${seeded} rows, expected ${ROWS}"
            fi
        fi
    fi

    # 4. The measured run. Nothing between the timer and the implementation.
    if [ "$status" = "ok" ]; then
        invoke_implementation "$impl" "$properties" "$migrate_log" "$summary_file" "$workload"
        cold="$RUN_SECONDS"
        rows_written="$RUN_ROWS_WRITTEN"
        if [ "$RUN_STATUS" != "ok" ]; then
            status="failed"
            note="the migration did not succeed: ${RUN_STATUS}"
        fi
    fi

    # 5. Verification.
    #
    #    REQUIREMENT: both runs must be shown to have produced the same rows, by something that is
    #    not the job's own accounting. Two independent checks: a `SELECT COUNT(*)` issued by cqlsh,
    #    which agrees with the job's write counter or the run is not reportable; and a full
    #    `cdm validate`, which compares both sides row by row and column by column. Note in passing
    #    that validate is cdm-rs's comparator on both sides — it is fully independent of Java CDM's
    #    writer and independent of cdm-rs's *migrate* path, but not of cdm-rs itself. The COUNT(*)
    #    is the part that depends on neither implementation.
    if [ "$status" = "ok" ]; then
        target_rows="$(count_rows target "$(harness_table_for "$workload" target)")"
        if [ "$target_rows" != "$ROWS" ] || [ "$rows_written" != "$ROWS" ]; then
            status="unverified"
            note="seeded ${ROWS}, the job counted ${rows_written} written, the target holds ${target_rows}"
            # Name the cause when the job admitted to one. Java CDM's most likely reason for a
            # short target is write failures it exited 0 on; saying so here saves the reader
            # correlating this against a multi-megabyte Spark log.
            if [ "${RUN_ERROR_RECORDS:-null}" != "null" ] && [ "${RUN_ERROR_RECORDS:-0}" -gt 0 ]; then
                note="${note}; the job reported ${RUN_ERROR_RECORDS} error records"
            fi
        fi
    fi
    if [ "$status" = "ok" ]; then
        if "$CDM_BIN" validate --properties-file "$properties" --summary-out "$validate_summary" \
                > "$validate_log" 2>&1; then
            validate_status="clean"
            validate_discrepancies=0
        else
            validate_discrepancies="$(jq -r '
                ((.counters.MISMATCH // 0) + (.counters.MISSING // 0)
                 - (.counters.CORRECTED_MISMATCH // 0) - (.counters.CORRECTED_MISSING // 0))
            ' "$validate_summary" 2>/dev/null || printf 'null')"
            validate_status="differences"
            status="unverified"
            note="cdm validate reported ${validate_discrepancies} unrepaired difference(s)"
        fi
    fi

    # 6. Steady state, now that the cold figure is known.
    if [ -n "$cold" ] && [ -n "$startup" ]; then
        steady="$(fsub "$cold" "$startup")"
        # A startup floor as large as the run it is subtracted from means the run was dominated by
        # fixed cost, and the difference is noise rather than a steady state. Report nothing rather
        # than a near-zero denominator, which would produce a spectacular fake throughput.
        if [ "$(awk -v s="$steady" 'BEGIN { print (s <= 1.0) ? 1 : 0 }')" = "1" ]; then
            steady=""
            note="${note:+${note}; }steady state not reported: the startup floor accounts for the whole run"
        fi
    fi

    finished_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

    # 7. The result document.
    jq -n \
        --arg schema "cdm-rs.java-comparison.run/v1" \
        --arg workload "$workload" \
        --arg implementation "$impl" \
        --arg version "$(version_for "$impl")" \
        --arg status "$status" \
        --arg note "$note" \
        --argjson repeat "$repeat" \
        --argjson order_index "$order_index" \
        --argjson rows_expected "$ROWS" \
        --argjson rows_written "${rows_written:-null}" \
        --argjson origin_row_count "${seeded:-null}" \
        --argjson target_row_count "${target_rows:-null}" \
        --arg validate_status "$validate_status" \
        --argjson validate_discrepancies "${validate_discrepancies:-null}" \
        --argjson cold_secs "$(num_or_null "$cold")" \
        --argjson startup_secs "$(num_or_null "$startup")" \
        --argjson steady_secs "$(num_or_null "$steady")" \
        --argjson cold_rows_per_sec "$(frate "$ROWS" "${cold:-}")" \
        --argjson steady_rows_per_sec "$(frate "$ROWS" "${steady:-}")" \
        --arg started_at "$started_at" \
        --arg finished_at "$finished_at" \
        --arg properties_sha256 "$(sha256_of "$properties")" \
        --arg properties_equivalence "$( [ "$impl" = "cdm-rs" ] && printf 'file' || printf '%s' "$JAVA_PROPERTIES_EQUIVALENCE" )" \
        --slurpfile environment "$ENV_JSON" \
        '{schema: $schema, workload: $workload, implementation: $implementation, version: $version,
          status: $status, note: (if $note == "" then null else $note end),
          repeat: $repeat, order_index: $order_index,
          rows_expected: $rows_expected, rows_written: $rows_written,
          origin_row_count: $origin_row_count, target_row_count: $target_row_count,
          verification: {method: "independent SELECT COUNT(*) on the target, plus a full cdm validate",
                         validate: $validate_status,
                         unrepaired_differences: $validate_discrepancies},
          verified: ($status == "ok"),
          cold_wall_clock_secs: $cold_secs, startup_secs: $startup_secs,
          steady_state_secs: $steady_secs,
          cold_rows_per_sec: $cold_rows_per_sec, steady_rows_per_sec: $steady_rows_per_sec,
          started_at: $started_at, finished_at: $finished_at,
          properties_sha256: $properties_sha256, properties_equivalence: $properties_equivalence,
          environment: $environment[0]}' > "$run_json"

    log "${impl}/${workload}: ${status}${note:+ — ${note}}"

    # 8. Down they come, before the other implementation gets its own pair.
    [ "$KEEP_CONTAINERS" -eq 1 ] || harness_clusters_down
}

# A run that never happened, recorded as such: an absence in the results directory rather than a
# gap, so that a reader of comparison.json can see the Java side was asked for and why it did not
# answer.
record_unavailable() {
    local impl="$1" workload="$2" repeat="$3" order_index="$4" reason="$5"
    jq -n \
        --arg schema "cdm-rs.java-comparison.run/v1" \
        --arg workload "$workload" --arg implementation "$impl" \
        --arg status "unavailable" --arg note "$reason" \
        --argjson repeat "$repeat" --argjson order_index "$order_index" \
        --argjson rows_expected "$ROWS" \
        --slurpfile environment "$ENV_JSON" \
        '{schema: $schema, workload: $workload, implementation: $implementation,
          version: null, status: $status, note: $note, repeat: $repeat, order_index: $order_index,
          rows_expected: $rows_expected, rows_written: null,
          origin_row_count: null, target_row_count: null,
          verification: {method: null, validate: "not run", unrepaired_differences: null},
          verified: false,
          cold_wall_clock_secs: null, startup_secs: null, steady_state_secs: null,
          cold_rows_per_sec: null, steady_rows_per_sec: null,
          started_at: null, finished_at: null,
          properties_sha256: null, properties_equivalence: null,
          environment: $environment[0]}' \
        > "${OUT_DIR}/runs/${workload}-${impl}-r${repeat}.json"
}

# --- order ---------------------------------------------------------------------------------------------
#
# REQUIREMENT: alternate the order across runs, or make it a parameter and record it. Both.
#
# `--first` fixes the starting side; `auto` alternates on the repeat index, so `--repeats 2` covers
# both orders within one invocation. A single-repeat scheduled run has nothing to alternate
# *within*, so the starting side comes from the ISO week number: consecutive fortnightly runs
# therefore swap, and any residual order effect shows up in the trend as an oscillation instead of
# being baked into every run in the same direction.
#
# `order_index` (0 = ran first, 1 = ran second) is on every result either way, so the question can
# always be asked of the data afterwards.
order_for_repeat() {
    local repeat="$1" base
    case "$FIRST" in
        cdm-rs)   base=0 ;;
        java-cdm) base=1 ;;
        *)        base=$(( 10#$(date -u +%V) % 2 )) ;;
    esac
    if [ $(( (base + repeat) % 2 )) -eq 0 ]; then
        printf 'cdm-rs java-cdm'
    else
        printf 'java-cdm cdm-rs'
    fi
}

# --- the sweep ------------------------------------------------------------------------------------------

# A container left running holds 1–2 GiB of RAM after the job that started it has finished, so
# teardown happens on the way out however the script leaves.
# shellcheck disable=SC2329  # invoked by the trap below, which shellcheck does not follow
on_exit() {
    [ "$KEEP_CONTAINERS" -eq 1 ] || harness_clusters_down
}
trap on_exit EXIT INT TERM

for workload in "${WORKLOADS[@]}"; do
    if [ ! -r "$(harness_properties_for "$workload")" ]; then
        warn "skipping ${workload}: no properties file at $(harness_properties_for "$workload")"
        continue
    fi
    for (( repeat = 0; repeat < REPEATS; repeat++ )); do
        order_index=0
        for impl in $(order_for_repeat "$repeat"); do
            if [ "$impl" = "java-cdm" ] && [ "$JAVA_STATUS" != "ok" ]; then
                record_unavailable "$impl" "$workload" "$repeat" "$order_index" "$JAVA_UNAVAILABLE_REASON"
            else
                run_one "$impl" "$workload" "$repeat" "$order_index"
            fi
            order_index=$(( order_index + 1 ))
        done
    done
done

# --- aggregation -----------------------------------------------------------------------------------------
#
# A ratio is emitted only when BOTH sides have a verified run for that workload. Anything else — a
# failed Java run, an unverified target, a missing steady state — produces a null ratio and a
# sentence naming what is missing. A number computed against a partial run is not a weaker result,
# it is a wrong one.

COMPARISON_JSON="${OUT_DIR}/comparison.json"
RESULTS_MD="${OUT_DIR}/results.md"

# The runs directory can legitimately hold nothing (every workload skipped, or every run refused),
# and it also holds each run's `MET-033` summary. So: guard the glob, and keep only the documents
# that are this schema. An unguarded glob under `pipefail` ends the script here, silently, with no
# results file — which is how a harness comes to report nothing at all.
if compgen -G "${OUT_DIR}/runs/*.json" > /dev/null; then
    ALL_RUNS="$(jq -s '[.[] | select(.schema? == "cdm-rs.java-comparison.run/v1")]' "${OUT_DIR}"/runs/*.json)"
else
    ALL_RUNS='[]'
fi

jq -n \
    --arg schema "cdm-rs.java-comparison/v1" \
    --arg generated_at "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
    --argjson rows "$ROWS" \
    --argjson repeats "$REPEATS" \
    --arg order_policy "$FIRST" \
    --slurpfile environment "$ENV_JSON" \
    --argjson runs "$ALL_RUNS" \
    '
    def med($xs): ($xs | sort) as $s | ($s | length) as $n |
        if $n == 0 then null
        elif $n % 2 == 1 then $s[($n / 2 | floor)]
        else (($s[$n / 2 - 1] + $s[$n / 2]) / 2) end;

    def rate($rs; $impl; $field):
        med([$rs[] | select(.implementation == $impl and .verified and .[$field] != null) | .[$field]]);

    ($runs | map(.workload) | unique) as $workloads |
    {
      schema: $schema,
      generated_at: $generated_at,
      rows: $rows,
      repeats: $repeats,
      order_policy: $order_policy,
      aggregate: (if $repeats > 1
                  then "median over the verified runs of each side, computed identically for both"
                  else "one verified run per side" end),
      environment: $environment[0],
      comparisons: [ $workloads[] as $w |
        ($runs | map(select(.workload == $w)))   as $rs |
        (rate($rs; "cdm-rs";   "cold_rows_per_sec"))   as $rust_cold   |
        (rate($rs; "cdm-rs";   "steady_rows_per_sec")) as $rust_steady |
        (rate($rs; "java-cdm"; "cold_rows_per_sec"))   as $java_cold   |
        (rate($rs; "java-cdm"; "steady_rows_per_sec")) as $java_steady |
        {
          workload: $w,
          cdm_rs:   {cold_rows_per_sec: $rust_cold, steady_rows_per_sec: $rust_steady,
                     runs: [$rs[] | select(.implementation == "cdm-rs")   | {status, verified, note}]},
          java_cdm: {cold_rows_per_sec: $java_cold, steady_rows_per_sec: $java_steady,
                     runs: [$rs[] | select(.implementation == "java-cdm") | {status, verified, note}]},
          cold_ratio:   (if ($rust_cold != null and $java_cold != null and $java_cold > 0)
                         then ($rust_cold / $java_cold) else null end),
          steady_ratio: (if ($rust_steady != null and $java_steady != null and $java_steady > 0)
                         then ($rust_steady / $java_steady) else null end),
          note: (if $java_cold == null then "no verified Java CDM run: no ratio is reported"
                 elif $rust_cold == null then "no verified cdm-rs run: no ratio is reported"
                 else null end)
        }
      ],
      runs: $runs
    }' > "$COMPARISON_JSON"

# The human-readable table is rendered *from* comparison.json rather than computed a second time,
# so the prose and the machine-readable document cannot disagree about what was measured.
{
    printf '%s\n\n' "# Java CDM vs cdm-rs — tier 3 (\`NFR-004\`)"
    jq -r '
        "Generated \(.generated_at) • rows \(.rows) • repeats \(.repeats) • order policy \(.order_policy)",
        "",
        "Aggregate: \(.aggregate).",
        "",
        "Runner: \(.environment.cpus) vCPU, \((.environment.memory_bytes / 1073741824) | floor) GiB RAM, " +
        "\((.environment.disk_free_bytes / 1073741824) | floor) GiB free disk, " +
        "\(.environment.os) \(.environment.kernel) \(.environment.arch), \(.environment.cpu_model).",
        "",
        "cdm-rs \(.environment.cdm_rs_version) @ \(.environment.cdm_rs_commit[0:12]) • " +
        "Java CDM \(.environment.java_cdm) • \(.environment.spark) • \(.environment.java) • " +
        "\(.environment.cassandra_image), heap \(.environment.cassandra_heap) per node",
        "",
        "| Workload | Implementation | Cold rows/s | Steady rows/s | Run status |",
        "|---|---|---:|---:|---|",
        (.comparisons[] | . as $c |
          "| \($c.workload) | cdm-rs | \($c.cdm_rs.cold_rows_per_sec // "not measured") | " +
          "\($c.cdm_rs.steady_rows_per_sec // "not measured") | " +
          "\([$c.cdm_rs.runs[] | .status] | join(", ")) |",
          "| \($c.workload) | Java CDM | \($c.java_cdm.cold_rows_per_sec // "not measured") | " +
          "\($c.java_cdm.steady_rows_per_sec // "not measured") | " +
          "\([$c.java_cdm.runs[] | .status] | join(", ")) |"),
        "",
        "| Workload | Cold ratio | Steady ratio | Note |",
        "|---|---:|---:|---|",
        (.comparisons[] |
          "| \(.workload) | " +
          "\(if .cold_ratio == null then "not reported" else "\((.cold_ratio * 100 | round) / 100)x" end) | " +
          "\(if .steady_ratio == null then "not reported" else "\((.steady_ratio * 100 | round) / 100)x" end) | " +
          "\(.note // "") |")
    ' "$COMPARISON_JSON"
    cat <<'EOF'

Cold includes process start, JVM boot, SparkSession creation and executor startup. Steady state
subtracts a separately measured startup floor — the same workload with zero rows in it — from the
cold figure, by the same procedure on both sides. Both are honest; they answer different questions.
A ratio is printed only where both sides produced a verified run. See `docs/BENCHMARKS.md` §5.
EOF
} > "$RESULTS_MD"

log "results:          ${RESULTS_MD}"
log "machine-readable: ${COMPARISON_JSON}"
cat "$RESULTS_MD"

# Deliberately zero, whatever the ratio was. This harness reports; it does not judge.
exit 0
