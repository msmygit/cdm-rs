#!/usr/bin/env bash
#
# Start the origin and target Cassandra nodes for the tier-3 comparison (`NFR-004`, `TST-060`).
#
# These two containers must be indistinguishable from the ones `crates/cdm-testkit/src/containers.rs`
# starts for cdm-rs, because the entire claim being measured is "same hardware, same data, same
# databases, two implementations". Image tag, heap and new-size therefore come from
# `versions.env`, which takes them from that file's `DEFAULT_ENGINE` and `DEFAULT_HEAP_MIB`.
#
# Two details are inherited from `containers.rs` and are not stylistic:
#
#   * The heap is capped explicitly, and `MAX_HEAP_SIZE`/`HEAP_NEWSIZE` are set as a pair.
#     Unbounded, each node sizes its heap from machine RAM and commits it with
#     `-XX:+AlwaysPreTouch` -- ~4 GiB per node on a 16 GiB box. Two of those plus a Spark driver
#     holding 6 GiB does not fit, and the failure mode is the origin being OOM-killed by the
#     kernel the moment the target starts, which reads as a mysterious connection refusal.
#     `cassandra-env.sh` aborts if only one of the pair is set under CMS, so both are set even
#     though 5.0 runs G1 and ignores `HEAP_NEWSIZE`.
#
#   * Readiness is proved by a query, not by the log line. "Startup complete" is written before
#     the native transport binds, so a script that trusts it hands the next step a node that
#     refuses the connection. `containers.rs` sends a native-protocol OPTIONS frame because it may
#     not depend on a CQL driver; here `cqlsh` ships inside the image, so the probe is simply a
#     `SELECT` against `system.local`, which proves more.
#
# Usage:
#   bench/java-comparison/environment/clusters-up.sh
#
# Idempotent: an already-running pair is left alone. Exit status is 0 once both nodes answer CQL.

set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=versions.env
. "${HERE}/versions.env"

# How long a node gets to bind its native transport. A cold `cassandra:5.0` on a 4-vCPU runner
# takes 45-75 s; the ceiling is generous because the cost of waiting too long is a slow script and
# the cost of waiting too little is an intermittently red benchmark.
READY_TIMEOUT_SECONDS=300

log() { printf '[clusters-up] %s\n' "$*"; }

# A user-defined bridge with a fixed subnet, not the default bridge. Cassandra advertises an
# address in `system.local.rpc_address` and every driver honours it when it builds its connection
# pool, so that address has to be one the client can route to -- and it must be stable across
# restarts, or the properties file goes stale. Docker's default bridge hands out whatever is free;
# a user-defined network lets each container be pinned with `--ip`.
if ! docker network inspect "${BENCH_NETWORK}" > /dev/null 2>&1; then
    log "creating network ${BENCH_NETWORK} (${BENCH_SUBNET})"
    docker network create --subnet "${BENCH_SUBNET}" "${BENCH_NETWORK}" > /dev/null
fi

start_node() {
    local name="$1" ip="$2" cluster="$3"

    if docker ps --format '{{.Names}}' | grep -qx "${name}"; then
        log "${name} already running"
        return 0
    fi
    # A stopped container of the same name would make `docker run` fail with a name clash, and its
    # data directory is stale by definition -- the workload loader will have moved on.
    docker rm -f "${name}" > /dev/null 2>&1 || true

    log "starting ${name} at ${ip} (${CASSANDRA_IMAGE}, heap ${CASSANDRA_HEAP_MIB}M)"
    # No volume, and no `--memory` cap. No volume because the dataset is regenerated per run and a
    # named volume merely survives to be stale. No `--memory` because the JVM heap cap above is
    # the control that matters and a cgroup limit on top of it converts a heap-pressure slowdown
    # into an opaque kill.
    docker run --detach \
        --name "${name}" \
        --network "${BENCH_NETWORK}" \
        --ip "${ip}" \
        --env "CASSANDRA_CLUSTER_NAME=${cluster}" \
        --env "CASSANDRA_BROADCAST_RPC_ADDRESS=${ip}" \
        --env "MAX_HEAP_SIZE=${CASSANDRA_HEAP_MIB}M" \
        --env "HEAP_NEWSIZE=${CASSANDRA_NEWSIZE_MIB}M" \
        "${CASSANDRA_IMAGE}" > /dev/null
}

wait_for_cql() {
    local name="$1" ip="$2"
    local deadline=$((SECONDS + READY_TIMEOUT_SECONDS))

    log "waiting for ${name} to answer CQL on ${ip}:${NATIVE_PORT}"
    while [ "${SECONDS}" -lt "${deadline}" ]; do
        # Run cqlsh *inside* the node's own container: it is the one place the client is known to
        # exist and to match the server version, and it needs no host-side install.
        if docker exec "${name}" cqlsh "${ip}" "${NATIVE_PORT}" \
            -e 'SELECT release_version FROM system.local' > /dev/null 2>&1; then
            log "${name} is ready"
            return 0
        fi
        # If the container died -- almost always the heap not fitting -- say so now rather than
        # spending the whole timeout polling a corpse.
        if ! docker ps --format '{{.Names}}' | grep -qx "${name}"; then
            log "${name} exited during startup; last 20 log lines:"
            docker logs --tail 20 "${name}" >&2 || true
            return 1
        fi
        sleep 3
    done

    log "${name} did not answer CQL within ${READY_TIMEOUT_SECONDS}s; last 20 log lines:"
    docker logs --tail 20 "${name}" >&2 || true
    return 1
}

# Distinct cluster names. Both nodes sit on one bridge network with gossip reachable between them,
# and two single-node clusters that share a name are an invitation for one to try to join the
# other. Different names make Cassandra reject the handshake outright, which is the desired
# outcome: these are two independent clusters that happen to be neighbours.
start_node "${ORIGIN_CONTAINER}" "${ORIGIN_IP}" cdm-bench-origin
start_node "${TARGET_CONTAINER}" "${TARGET_IP}" cdm-bench-target

wait_for_cql "${ORIGIN_CONTAINER}" "${ORIGIN_IP}"
wait_for_cql "${TARGET_CONTAINER}" "${TARGET_IP}"

log "origin ${ORIGIN_IP}:${NATIVE_PORT}  target ${TARGET_IP}:${NATIVE_PORT}"
