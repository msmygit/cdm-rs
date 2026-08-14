# Java CDM 6.0.1 on Spark 4.1.2 — the tier-3 comparison baseline (`NFR-004`, `TST-060`).
#
# Every version here arrives as a build argument from `versions.env` via `build-image.sh`. There
# are no defaults on purpose: an unpinned build of a benchmark baseline is not a baseline, and a
# `Dockerfile` that silently falls back to "latest" would hide exactly the Spark/Scala drift this
# whole exercise exists to control.
#
# Why not `datastax/cassandra-data-migrator:6.0.1` from Docker Hub? It exists, it is official, and
# it would work. It is also 870 MiB compressed / ~2.4 GiB on disk, because it bundles `dsbulk` and
# `cqlsh` alongside CDM, and its contents are not visible in this repository. This image is ~1.3
# GiB, fetches two artifacts whose SHA-512 sums are recorded in `versions.env`, and can be audited
# by reading twenty lines. On a runner with ~14 GiB free, ~1 GiB matters. See
# README-ENVIRONMENT.md, "Why not the official image".

ARG JDK_IMAGE
FROM ${JDK_IMAGE}

ARG SPARK_VERSION
ARG SPARK_TGZ
ARG SPARK_URL
ARG SPARK_SHA512
ARG CDM_VERSION
ARG CDM_JAR
ARG CDM_JAR_URL
ARG CDM_JAR_SHA512

# `curl` to fetch, `procps` because Spark's `bin/spark-class` shells out to `ps` when it computes
# the driver command line and prints a confusing failure without it. Nothing else: no `python3`
# (PySpark is not used and costs ~50 MiB), no build tools.
RUN apt-get update \
    && apt-get install --no-install-recommends -y ca-certificates curl procps \
    && rm -rf /var/lib/apt/lists/*

# Spark and the CDM jar in one layer, so the 573 MiB tarball never reaches the final image. The
# checksums are the point of this step: a truncated or substituted download must fail the build
# loudly rather than surface hours later as an unrepeatable benchmark number.
RUN set -eux; \
    curl -fsSL -o "/tmp/${SPARK_TGZ}" "${SPARK_URL}"; \
    echo "${SPARK_SHA512}  /tmp/${SPARK_TGZ}" | sha512sum -c -; \
    mkdir -p /opt/spark; \
    tar -xzf "/tmp/${SPARK_TGZ}" -C /opt/spark --strip-components=1; \
    rm -f "/tmp/${SPARK_TGZ}"; \
    # Spark ships R and Python bindings and its own example jars. None are reachable from a
    # `spark-submit --class com.datastax.cdm.job.Migrate` run, and together they are ~180 MiB.
    rm -rf /opt/spark/R /opt/spark/python /opt/spark/examples /opt/spark/data; \
    mkdir -p /opt/cdm; \
    curl -fsSL -o "/opt/cdm/${CDM_JAR}" "${CDM_JAR_URL}"; \
    echo "${CDM_JAR_SHA512}  /opt/cdm/${CDM_JAR}" | sha512sum -c -

# Recorded in the image so that `docker run ... env | grep CDM_` answers "which versions was this
# number produced with" without reference to the tree that built it.
ENV SPARK_HOME=/opt/spark \
    PATH=/opt/spark/bin:/opt/spark/sbin:${PATH} \
    CDM_JAR_PATH=/opt/cdm/${CDM_JAR} \
    CDM_VERSION=${CDM_VERSION} \
    SPARK_VERSION=${SPARK_VERSION}

# Spark writes shuffle spill, event logs and the `local[*]` scratch directory under whatever
# `spark.local.dir` says; the default is `/tmp`, which is fine, but the working directory needs to
# be writable by an arbitrary `--user` so that files the run produces are not root-owned on a
# bind mount.
WORKDIR /work

# No ENTRYPOINT. The `spark-submit` line is a deliverable in its own right and lives in
# `submit-migrate.sh`, where it is visible and editable, not buried in an image.
CMD ["spark-submit", "--version"]
