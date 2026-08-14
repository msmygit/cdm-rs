#!/usr/bin/env bash
#
# Stop and remove the tier-3 comparison's Cassandra nodes and their network (`NFR-004`, `TST-060`).
#
# Deliberately separate from `clusters-up.sh` rather than a trap inside it: a failed run's nodes
# are the only place the evidence lives (`docker logs`, `nodetool tablestats`), so teardown is
# something the operator asks for once they are done looking.
#
# Usage:
#   bench/java-comparison/environment/clusters-down.sh
#
# Removes nothing it did not create, and succeeds if there was nothing to remove.

set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=versions.env
. "${HERE}/versions.env"

log() { printf '[clusters-down] %s\n' "$*"; }

for name in "${ORIGIN_CONTAINER}" "${TARGET_CONTAINER}"; do
    if docker ps --all --format '{{.Names}}' | grep -qx "${name}"; then
        log "removing ${name}"
        docker rm --force "${name}" > /dev/null
    fi
done

# The network only disappears once nothing is attached, and a stray Spark container from an
# interrupted submit will hold it. Report that rather than failing: the containers are the part
# that costs RAM, and they are already gone.
if docker network inspect "${BENCH_NETWORK}" > /dev/null 2>&1; then
    if docker network rm "${BENCH_NETWORK}" > /dev/null 2>&1; then
        log "removed network ${BENCH_NETWORK}"
    else
        log "network ${BENCH_NETWORK} still has endpoints attached; left in place"
    fi
fi

log "done. The image ${BENCH_IMAGE} is left in place; remove it with:"
log "  docker rmi ${BENCH_IMAGE} ${CASSANDRA_IMAGE}"
