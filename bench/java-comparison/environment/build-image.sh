#!/usr/bin/env bash
#
# Build the Java CDM + Spark image for the tier-3 comparison (`NFR-004`, `TST-060`).
#
# From nothing, on a clean Linux box with only Docker and a network, this is the first command to
# run. It downloads ~600 MiB (Spark 4.1.2 plus the CDM 6.0.1 jar), verifies both against the
# SHA-512 sums pinned in `versions.env`, and produces a ~1.3 GiB local image.
#
# Usage:
#   bench/java-comparison/environment/build-image.sh            # build if absent
#   bench/java-comparison/environment/build-image.sh --force    # rebuild even if present
#
# Runs from any working directory. Exit status is 0 on success.

set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=versions.env
. "${HERE}/versions.env"

FORCE=0
while [ $# -gt 0 ]; do
    case "$1" in
        --force) FORCE=1 ;;
        -h | --help)
            sed -n '3,13p' "$0" | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        *)
            printf 'unknown argument: %s\n' "$1" >&2
            exit 2
            ;;
    esac
    shift
done

# The image tag carries both component versions, so "already built" can only ever mean "built from
# the versions currently pinned". Changing a pin changes the tag and forces a rebuild by itself;
# --force exists for the case where only the Dockerfile changed.
if [ "${FORCE}" -eq 0 ] && docker image inspect "${BENCH_IMAGE}" > /dev/null 2>&1; then
    printf '%s already present; pass --force to rebuild\n' "${BENCH_IMAGE}"
    exit 0
fi

# `--platform linux/amd64` because that is what the runner is and what the artifacts are. Spark's
# tarball is pure JVM bytecode and would run anywhere, but the CDM jar carries
# `netty-transport-native-epoll` classifiers, and an Apple Silicon developer silently building an
# arm64 image would be measuring a different stack than CI does. Being explicit costs nothing and
# removes the question.
docker build \
    --platform linux/amd64 \
    --tag "${BENCH_IMAGE}" \
    --build-arg "JDK_IMAGE=${JDK_IMAGE}" \
    --build-arg "SPARK_VERSION=${SPARK_VERSION}" \
    --build-arg "SPARK_TGZ=${SPARK_TGZ}" \
    --build-arg "SPARK_URL=${SPARK_URL}" \
    --build-arg "SPARK_SHA512=${SPARK_SHA512}" \
    --build-arg "CDM_VERSION=${CDM_VERSION}" \
    --build-arg "CDM_JAR=${CDM_JAR}" \
    --build-arg "CDM_JAR_URL=${CDM_JAR_URL}" \
    --build-arg "CDM_JAR_SHA512=${CDM_JAR_SHA512}" \
    "${HERE}"

printf '\nbuilt %s\n' "${BENCH_IMAGE}"
docker run --rm "${BENCH_IMAGE}" spark-submit --version 2>&1 | sed 's/^/  /'
