# The Java CDM baseline environment

Everything needed to stand up **Java Cassandra Data Migrator 6.0.1 on Apache Spark 4.1.2** and run
a real migration between two containerised Cassandra nodes, on a free GitHub Actions
`ubuntu-latest` runner.

This is tier 3 of `docs/BENCHMARKS.md` — the half of `NFR-004` that asks *"is cdm-rs really ≥ 2×
Java CDM?"*. This document covers only the **baseline environment**: the versions, the containers,
the `spark-submit` line and what they cost. The dataset, the comparison procedure and the numbers
themselves belong to `workloads/`, `run.sh` and `METHODOLOGY.md`.

Requirements: `NFR-004`, `TST-060`.

**It works.** A 200,000-row migration has been run end to end, verified by counting rows on the
target cluster, and is recorded in §6. Every workaround and every caveat is in §7 — there are
eight, and two of them (Java CDM exiting 0 after losing data, and the rate limiter doubling as the
only backpressure) will change how `run.sh` and `METHODOLOGY.md` have to be written.

---

## 1. Version matrix

| Component | Version | Where the number comes from |
|---|---|---|
| Java CDM | **6.0.1** | Latest `6.0.x`; `6.0.x` is the baseline cdm-rs supersedes (`docs/SPEC.md`) |
| Spark | **4.1.2** | CDM 6.0.1 `pom.xml` `<spark.version>`, and its README's "Prerequisite CDM 6.x+" |
| Scala | **2.13** | CDM 6.0.1 `pom.xml` `<scala.version>2.13.18</scala.version>`; the Spark 4.1.2 distribution ships 2.13.17 |
| JDK | **Temurin 17.0.19+10, JRE** | CDM 6.x requires "Java17 (minimum) as Spark 4.x binaries are compiled with it" |
| Spark Cassandra Connector | **3.5.1** | CDM 6.0.1 `pom.xml`, shaded into the jar — not fetched separately |
| Java driver | **4.19.3** | CDM 6.0.1 `pom.xml`, shaded into the jar |
| Cassandra | **5.0** | `crates/cdm-testkit/src/containers.rs` — the same image cdm-rs's own tests use |

All of it is Apache 2.0 and fetched from GitHub Releases, `archive.apache.org` and Docker Hub. No
accounts, no secrets, no licensed artifacts.

The single source of truth is [`environment/versions.env`](environment/versions.env), including
SHA-512 sums for both downloaded artifacts. Nothing else in this directory names a version.

### Why the Spark version is the whole task

cdm-rs's own README cites this as Java CDM's most common support ticket:

```
Exception in thread "main" java.lang.NoSuchMethodError: 'void scala.runtime.Statics.releaseFence()'
```

`scala.runtime.Statics.releaseFence` exists in Scala 2.13 and not in 2.12. A CDM jar compiled
against a 2.13 Spark, dropped onto a 2.12 Spark distribution, links fine and then dies on the first
RDD touch — minutes into a run, after the cluster connections are up, which is exactly when it is
least obvious that the cause is a classpath.

Two things make this a non-issue here rather than a hazard:

1. **Spark 4.x ships only a Scala 2.13 build.** The 3.5.x line had both, and the archive filenames
   differ only by a `-scala2.13` suffix — one keystroke between a working and a broken stack. Spark
   4.1.2's `spark-4.1.2-bin-hadoop3.tgz` has no 2.12 counterpart to pick by mistake. Choosing CDM
   6.x over 5.x removes the failure mode structurally.
2. **The tarball is fetched by URL with its published SHA-512 checked at build time.** The version
   cannot drift without the tag changing and the build failing.

We did not encounter `releaseFence` at any point. The three failures we *did* hit are in §7, and
none of them was a Scala mismatch.

---

## 2. Quick start

From nothing, on a clean Linux box with Docker and a network:

```bash
bench/java-comparison/environment/build-image.sh    # ~600 MiB download, ~2 min
bench/java-comparison/environment/clusters-up.sh    # two cassandra:5.0 nodes, ~90 s
bench/java-comparison/environment/smoke-test.sh     # proves rows actually move
```

`smoke-test.sh` takes an optional row count (default 10,000). It creates its own keyspace, loads
it, truncates the target, migrates, and compares row counts across the two clusters — the
truncation is not decoration, since without it a rerun would pass its count check having moved
nothing.

Then, for a table the workload loader has populated:

```bash
bench/java-comparison/environment/submit-migrate.sh my_keyspace.my_table
```

and when finished:

```bash
bench/java-comparison/environment/clusters-down.sh
docker rmi cdm-bench/java-cdm:6.0.1-spark4.1.2 cassandra:5.0
```

| File | What it is |
|---|---|
| `environment/versions.env` | Every pinned version and checksum. Sourced by all scripts |
| `environment/Dockerfile` | Temurin 17 JRE + Spark 4.1.2 + the CDM 6.0.1 jar |
| `environment/build-image.sh` | Builds the image; verifies both downloads |
| `environment/clusters-up.sh` | Origin and target `cassandra:5.0`, capped heaps, fixed IPs, CQL readiness probe |
| `environment/clusters-down.sh` | Removes them and the network |
| `environment/cdm.properties.template` | CDM configuration, with every deviation from CDM's defaults argued in place |
| `environment/submit-migrate.sh` | **The `spark-submit` invocation.** Writes a log and the exact properties used |
| `environment/smoke-test.sh` | Loads rows, migrates, counts the far side. The evidence in §6 |

---

## 3. The `spark-submit` invocation

This is the deliverable that matters most, so it is reproduced here in full. `submit-migrate.sh`
runs exactly this inside the image:

```bash
spark-submit \
  --driver-java-options "-Duser.home=/work/out" \
  --master "local[4]" \
  --driver-memory 4G \
  --conf spark.ui.enabled=false \
  --conf spark.local.dir=/work/out/spark-local \
  --conf spark.cdm.schema.origin.keyspaceTable=<keyspace.table> \
  --properties-file /work/out/cdm-<timestamp>.properties \
  --class com.datastax.cdm.job.Migrate \
  /opt/cdm/cassandra-data-migrator-6.0.1.jar
```

CDM's README suggests `--master "local[*]" --driver-memory 25G --executor-memory 25G`. That is
sized for a dedicated migration VM. Here it is wrong in three ways:

| Setting | CDM README | Here | Why |
|---|---|---|---|
| `--driver-memory` | `25G` | `4G` | 25 GiB does not exist on a 16 GiB runner. Requesting it either fails at JVM start or succeeds and gets a Cassandra container OOM-killed mid-run, which reads as a CDM bug. See §5 |
| `--executor-memory` | `25G` | *omitted* | In `local` mode there are no executors — the driver JVM **is** the executor and the flag is parsed and ignored. Passing it is cargo cult |
| `--master` | `local[*]` | `local[4]` | `local` mode kept: standalone on one box adds a master and a worker JVM (~500 MiB each) for no parallelism `local[N]` does not already provide. But **`4`, not `*`** — see §7.5, where `local[*]` on a 12-core host silently dropped 1,465 rows. On the reference runner the two are the same |

Plus two settings CDM's README does not mention:

- `spark.ui.enabled=false` — the UI binds a port, starts Jetty and retains stage data for the run.
  Useful when diagnosing, pure overhead when measuring. `SPARK_UI=true` puts it back.
- `spark.local.dir` on the bind mount — shuffle spill on the container's writable layer counts
  against Docker's storage, which on a 14 GiB runner is the disk that runs out.

### The `cdm.properties` settings that change the answer

Two entries in `environment/cdm.properties.template` will change the measured throughput by more
than anything in the Spark configuration.

**`spark.cdm.perfops.ratelimit.{origin,target}` defaults to `20000`, and it is a cap.** Not a
target, not a guardrail that opens under load — a hard ceiling of 20,000 rows/second per side. A
benchmark run against CDM's default would measure the limiter, produce a suspiciously round number,
and hand cdm-rs an arbitrarily large win.

It is equally wrong to set it to infinity, which is what we tried first. **The limiter is also the
only backpressure in CDM's write path.** At `1000000`, the same 200,000-row job reported
`Final Error Record Count: 135264` and left 24,409 rows missing on the target, drowning in
`NodeUnavailableException: No connection was available` — CDM fires unbounded async batches and
exhausts the Java driver's connection pool against a single-node target.

It is set to `100000` here: comfortably above the ~22,000 rows/second the hardware actually
achieves, and low enough to still throttle a burst. **This is the one number to check before
publishing any ratio** — confirm the measured rate is well under it for whatever dataset is chosen,
and raise it (and re-verify) if it is not.

**`spark.cdm.perfops.numParts` defaults to `5000`, and is set to `64`.** Each ring split is a Spark
task with a driver round trip and its own statement preparation. On a real cluster with billions of
rows, 5000 splits is right. On one node with a few million, the scheduler dominates. 64 is 16 tasks
per core on a 4-vCPU box: enough to keep every core busy through a straggler, few enough that
per-task cost stays under a percent. **This is a tuning choice made in Java CDM's favour and must
be declared in `METHODOLOGY.md`.** For datasets much larger than the smoke test, raise it —
`CDM_NUM_PARTS=512 submit-migrate.sh …`.

`spark.cdm.trackRun` is `false`. `true` makes CDM write a row per range to `cdm_run_info` and
`cdm_run_details` on the target, which is real work the throughput would include. cdm-rs has an
equivalent (`TRK-*`); whichever way it is set, **it must be set the same way on both sides**.

---

## 4. Networking

Cassandra advertises an address in `system.local.rpc_address`, and every driver honours it when it
builds its connection pool — the control connection succeeds on whatever address you dialled, and
then every pooled connection goes to the advertised one. `crates/cdm-testkit/src/containers.rs`
documents this at length: it is why that fixture publishes the container port on the *same* host
port and sets `broadcast_rpc_address` to `127.0.0.1`.

The Spark driver here runs in a container, not on the host, so it needs the opposite arrangement:

- a user-defined bridge network `cdm-bench-net` on `172.28.0.0/16`;
- origin pinned at `172.28.0.11`, target at `172.28.0.12`, each with
  `CASSANDRA_BROADCAST_RPC_ADDRESS` set to its own address;
- the Spark container attached to the same network.

Fixed addresses rather than container names because the value has to be an IP — a hostname in
`rpc_address` is not what the driver expects — and it has to survive a restart, or the generated
properties file goes stale.

**Consequence for `run.sh`:** on Linux, Docker bridge networks are routable from the host, so a
cdm-rs binary running on the host reaches `172.28.0.11:9042` directly and faces exactly the same
two nodes. **On Docker Desktop for macOS it does not**, because the bridge lives inside a VM. The
Java side is unaffected (its driver is in a container on that same bridge), which is why the
evidence in §6 was collected on a Mac; but a like-for-like comparison run needs Linux, which is the
reference environment anyway.

The two nodes are given **different `CASSANDRA_CLUSTER_NAME`s** deliberately. They share a network
and can see each other's gossip port; identical names would invite one to try to join the other,
and differing names make Cassandra reject the handshake outright. These are two independent
single-node clusters that happen to be neighbours.

### Keyspaces must be `NetworkTopologyStrategy`

CDM reads and writes at `LOCAL_QUORUM` by default, and `LOCAL_QUORUM` is defined over the replicas
in the *local datacenter*. `SimpleStrategy` has no datacenter concept. Workload keyspaces should
therefore be created as:

```sql
CREATE KEYSPACE ks WITH replication =
  {'class':'NetworkTopologyStrategy','datacenter1':1};
```

`datacenter1` is what a stock `cassandra:5.0` container calls its DC.

---

## 5. Disk and RAM budget

Measured on a Docker VM with 15.86 GiB of RAM and 12 CPUs, which is deliberately close to the
`ubuntu-latest` runner's 16 GiB / 4 vCPU.

### Disk — against ~14 GiB free

| Item | On disk | Note |
|---|---|---|
| `cdm-bench/java-cdm:6.0.1-spark4.1.2` | **1.58 GiB** | Temurin 17 JRE ~0.2, Spark 4.1.2 ~1.1 (after deleting R, Python, examples and data: ~180 MiB saved), CDM jar 23 MiB |
| `cassandra:5.0` | **0.54 GiB** | Pulled once, two containers share the layers |
| Build cache | ~1.6 GiB | Reclaimable: `docker builder prune -f` after the image is built |
| Spark log per run | 1–5 MiB | Kept as evidence |
| Shuffle scratch | < 100 MiB | `submit-migrate.sh` deletes it afterwards |
| **Fixed total** | **~2.2 GiB** | (~3.8 GiB before pruning the build cache) |

That leaves roughly **10 GiB for data**, and it has to hold the dataset **twice** — origin *and*
target — plus commitlog, plus the compaction headroom to actually write it. A safe planning figure
for the workload authors:

> **Budget ~4 GiB of on-disk SSTables per side.**

Measured on the smoke table (`id uuid PRIMARY KEY, customer text, amount int, note text`), from
the target node's own flush log: 117,927 rows → 12.86 MiB of serialised memtable → **7.66 MiB of
SSTable, i.e. ~68 bytes per row on disk**. Four GiB of that is roughly **60 million rows** of this
shape — so disk is not the binding constraint for any plausible dataset, and a wider table with
larger values is what would make it one.

Something in the low millions leaves a comfortable margin and still runs long enough that the
10-second JVM start-up floor and the JIT warm-up stop dominating, which matters more than disk
(§8).

If the download itself is a concern: the image build pulls ~600 MiB (Spark 573 MiB + CDM jar 23
MiB) plus the base image, and none of it is cached between CI jobs unless you cache it deliberately.

### RAM — against 16 GiB

| Process | Configured | Observed RSS | Note |
|---|---|---|---|
| Origin `cassandra:5.0` | 1 GiB heap (`MAX_HEAP_SIZE=1024M`, `HEAP_NEWSIZE=256M`) | **1.82 GiB** | Matches `containers.rs`'s `DEFAULT_HEAP_MIB` |
| Target `cassandra:5.0` | same | **1.74 GiB** | |
| Spark driver (`local[*]`) | `--driver-memory 4G` | ≤ 4 GiB + ~0.5 GiB metaspace/native | The driver *is* the executor in local mode |
| **Total** | | **~8 GiB** | Leaves ~8 GiB for the page cache, the kernel and the runner's own agent |

RSS runs ~0.8 GiB above the heap cap per node: off-heap memtables, the netty direct buffers, the
bloom filters and the JVM's own footprint. Budget for it — that is the gap that turns "1 + 1 + 4 =
6, fits fine" into an OOM kill.

**The heap cap is not optional.** Left alone, `cassandra:5.0` sizes its heap from machine RAM and
commits it with `-XX:+AlwaysPreTouch` — about 4 GiB per node on a 16 GiB box. Two of those plus a
4 GiB Spark driver does not fit, and the failure mode is the *origin* being OOM-killed by the
kernel the moment the target starts: a mysterious connection refusal with nothing wrong in either
node's log. `MAX_HEAP_SIZE` and `HEAP_NEWSIZE` are set as a pair because `cassandra-env.sh` aborts
with *"please set or unset MAX_HEAP_SIZE and HEAP_NEWSIZE in pairs"* when only one is set under CMS
— 5.0 runs G1 and ignores `HEAP_NEWSIZE`, but the pair is the spelling that works across the whole
3.11/4.0/4.1/5.0 matrix, which is what `containers.rs` does too.

If a run needs to be squeezed, `--driver-memory 3G` is safe for this workload shape; CDM streams
range by range and never collects to the driver. Below that the JVM starts spending its time in GC
and you are measuring the allocator.

---

## 6. Evidence: it runs

`smoke-test.sh` loads rows into origin, truncates target, runs `Migrate`, and counts the far side.
Truncation is not decoration — without it a rerun would pass its row-count check having moved
nothing.

**Environment:** Docker Desktop 4.85.0 on macOS, `linux/amd64` VM, 12 CPUs, 15.86 GiB RAM. *Not*
the reference hardware, and these are not `NFR-004` numbers. They establish that the stack works
and gives an order of magnitude.

```
$ bench/java-comparison/environment/smoke-test.sh 200000
[smoke] creating cdm_smoke.orders on both clusters
[smoke] truncating both tables
[smoke] loading 200000 rows into origin
[smoke] origin holds 200000 rows; target holds 0 before migration
[smoke] running Java CDM Migrate
[submit] com.datastax.cdm.job.Migrate cdm_smoke.orders
[submit] image cdm-bench/java-cdm:6.0.1-spark4.1.2
[submit] driver-memory 4G, local[4], numParts 64, ratelimit 100000

[submit] exit 0 after 19s
[submit] CDM counters:
  26/08/14 13:15:00 INFO JobCounter: Final Read Record Count: 200000
  26/08/14 13:15:00 INFO JobCounter: Final Write Record Count: 200000
  26/08/14 13:15:00 INFO JobCounter: Final Skipped Record Count: 0
  26/08/14 13:15:00 INFO JobCounter: Final Error Record Count: 0
[smoke] target holds 200000 rows after migration (was 0)
[smoke] PASS: Java CDM 6.0.1 on Spark 4.1.2 migrated 200000 rows
```

200,000 rows read on origin, 200,000 written to target, zero errors, verified by an independent
`SELECT COUNT(*)` against the target cluster after a `TRUNCATE`. Table: `id uuid PRIMARY KEY,
customer text, amount int, note text`, `NetworkTopologyStrategy{datacenter1:1}` on both sides.

Timings, read out of the run's own log (`environment/out/cdm-20260814_091442.log`):

| Phase | From the log | Seconds |
|---|---|---|
| `spark-submit` launched → `SparkContext: Running Spark version 4.1.2` | 13:14:42 → 13:14:46 | **4** |
| `SparkContext` up → `Starting job: foreach at Migrate.scala:46` | 13:14:46 → 13:14:51 | **5** |
| Job start → `Final Read Record Count` | 13:14:51 → 13:15:00 | **9** |
| **Total wall clock** | | **19** |

So: **~22,000 rows/second while moving data, ~10,500 rows/second end to end, on a ~10-second
start-up floor.** Both are ceilings on nothing and floors on nothing — the box is a laptop with 12
cores, three times the runner's, hosting all three JVMs. Treat them as *"this works and it is in
the tens of thousands of rows per second, not the hundreds or the millions"*.

**That 10-second floor is the single most important thing for `METHODOLOGY.md` to handle** — JVM
start, `SparkContext`, Hadoop's security stack, two connection pools, schema discovery. It is
roughly constant, cdm-rs pays almost none of it, and it will dominate any dataset that finishes in
under a minute. See §8.

How completely it dominates: the same script at its 10,000-row default also takes **10 seconds**
— twenty times less data, the same wall clock, because none of that time was ever the migration.
A tier-3 dataset chosen at that scale would produce a speed ratio that is really a measurement of
`main()`.

---

## 7. Caveats — every workaround, in order

Three things had to be worked around. All three are in the scripts with a comment; they are
collected here because anyone reproducing this on a different base image will meet them again.

### 1. `basedir must be absolute: ?/.ivy2.5.2/local`

```
Exception in thread "main" java.lang.IllegalArgumentException: basedir must be absolute: ?/.ivy2.5.2/local
	at org.apache.spark.util.MavenUtils$.createRepoResolvers(MavenUtils.scala:158)
	at org.apache.spark.deploy.SparkSubmit.prepareSubmitEnvironment(SparkSubmit.scala:340)
```

`submit-migrate.sh` passes `--user "$(id -u):$(id -g)"` so nothing lands on the host bind mount
owned by root. That uid has no `/etc/passwd` entry, `getpwuid` fails, and the JVM sets `user.home`
to the literal string `?`. spark-submit builds an Ivy resolver from `user.home` unconditionally,
before it looks at `--class` and whether or not `--packages` was passed.

**Setting `HOME` does not fix it** — the JVM reads the passwd database, not the environment. Fixed
with `--driver-java-options "-Duser.home=/work/out"`, and structurally by workaround 3.

### 2. `Invalid UID, could not determine effective user`

```
Exception in thread "main" java.io.IOException: Invalid UID, could not determine effective user
Caused by: javax.security.auth.login.LoginException: java.lang.NullPointerException: invalid null input: name
	at org.apache.hadoop.security.UserGroupInformation$HadoopLoginContext.login(UserGroupInformation.java:2154)
```

Same root cause, different victim: Hadoop's `UserGroupInformation` logs in via JAAS during
`SparkContext` construction and gets a null username. Spark 4.x initialises Hadoop's security stack
in `local` mode with no Hadoop anywhere in the job.

### 3. The fix for both: a generated `/etc/passwd`

`submit-migrate.sh` writes a two-line passwd file containing the invoking uid and bind-mounts it
read-only at `/etc/passwd`. That repairs the lookup at source rather than patching each symptom,
and it is the thing to copy if you re-home this onto another base image.

The alternative — running the container as root — leaves root-owned logs on the host bind mount
that an unprivileged `run.sh` cannot rotate or delete, and on a CI runner that is a real nuisance.

### 4. Java CDM exits 0 after losing data — do not trust the exit status

This is the most important operational finding here, and it is a property of Java CDM rather than
of this environment.

A `Migrate` run that reported

```
Final Read Record Count: 200000
Final Write Record Count: 165546
Final Error Record Count: 34454
```

and left **1,465 rows missing on the target** exited **0**, with `spark.cdm.perfops.errorLimit`
set to CDM's own default of `0`. `docs/MIGRATION_FROM_JAVA.md` item 42 records the same shape for
`DiffData`: nothing in `src/main/scala` calls `System.exit`, so `spark-submit` succeeds unless the
job throws.

A harness that times `spark-submit` and checks `$?` will therefore happily record a run that
dropped a sixth of the data — and, being a *shorter* run, record it as a *faster* one.
`submit-migrate.sh` consequently parses `Final Error Record Count` out of the log and exits 1 if it
is non-zero, which is deliberately stricter than CDM itself. **`run.sh` must do the same, or
compare row counts on both clusters afterwards, or both.**

### 5. `local[*]` on a big host silently loses rows

The run above was not a fluke and not a CDM bug so much as a capacity mismatch. At `--master
local[*]` on a 12-core host against a single-node target with a 1 GiB heap, 12 task threads firing
unbounded async batches exhaust the driver's connection pool:

```
com.datastax.oss.driver.api.core.AllNodesFailedException: All 1 node(s) tried for the query failed
  Node(endPoint=/172.28.0.12:9042): [NodeUnavailableException: No connection was available ...]
	at com.datastax.cdm.job.CopyJobSession.flushAndClearWrites(CopyJobSession.java:166)
```

Reproduced three times; `local[4]` on the same host, same dataset, same everything else is clean at
0 errors. The default is therefore `LOCAL_THREADS=4`, which is what the reference runner has
anyway. **Raising it requires raising the target's capacity too**, and re-checking the error count.

This interacts with the rate limiter: more threads and a higher limit are the same mistake twice.

### 6. Loading data: use `COPY FROM`, not batched `INSERT`s

Not a CDM issue, but it cost an hour. The first `smoke-test.sh` generated `BEGIN UNLOGGED BATCH`
blocks and piped them to `cqlsh`. Fine at 2,000 rows; at 100,000 it died around statement 10,500
with `NoHostAvailable: Connection to 172.28.0.11:9042 was closed`. One cqlsh connection fed batches
as fast as a shell can print them will trip a coordinator. `cqlsh COPY … FROM` is multi-process and
chunked with its own retries, and is what an operator would reach for. Worth knowing in
`workloads/`.

### 7. Not verified on a real GitHub Actions runner

This was built and run on Docker Desktop for macOS against a `linux/amd64` VM with 12 CPUs and
15.86 GiB. The RAM matches `ubuntu-latest`; the CPU count does not (12 vs 4) and the disk is not
constrained the way a runner's is. The disk budget in §5 is arithmetic from measured sizes, not an
observed near-miss. **Nothing here has been executed on a real runner**, and the first CI run
should be treated as the real test of the disk figure.

### 8. `linux/amd64` is pinned

`build-image.sh` passes `--platform linux/amd64`. Spark is bytecode and would run anywhere, but the
CDM jar carries `netty-transport-native-epoll` natives, and an Apple Silicon developer silently
building an arm64 image would be measuring a different stack than CI. On an M-series Mac the build
and the run go through emulation and are slow; that is a correctness choice, not an oversight.

---

## 8. What this does *not* establish, and what `METHODOLOGY.md` must handle

- **The JVM start-up floor is not free and is not a fair thing to hide.** 10 s of the 19-second
  smoke run is process start, `SparkContext`, Hadoop's login and connection-pool warm-up before a
  single row moves. cdm-rs, as a static binary, pays roughly none of it. That difference is *real*
  and is part
  of what `NFR-004` is claiming — but if the dataset is small enough that start-up dominates, the
  measured ratio is a measurement of `main()`, not of the migration engine. Two defensible
  readings, and `METHODOLOGY.md` should report **both**: total wall clock including start-up (what
  an operator experiences), and steady-state rows/second from CDM's own counters (what the engine
  does). A dataset that runs for at least a few minutes makes the choice matter less.
- **JIT warm-up.** A JVM that has just started is running interpreted. On a short run, Java CDM is
  measured before it reaches full speed. Same mitigation: run long enough.
- **`numParts=64` is a tuning choice in CDM's favour** and must be declared, along with a note that
  it was chosen for a single node and should scale with the dataset.
- **The rate limiter must be verified inert** for whatever dataset is chosen. If a run's rows/second
  lands suspiciously near a round number, check it before believing the ratio.
- **One run is not a result.** Nothing here repeats a measurement or reports variance. The four
  200,000-row runs made during this work took 15, 18, 21 and 19 seconds, and two of those were
  failures that finished *early*.
- **Correctness must be checked before speed is believed.** See §7.4: a faster run may simply be
  one that dropped more rows.
- **Validate (`DiffData`) is untested here.** `CDM_CLASS=com.datastax.cdm.job.DiffData
  submit-migrate.sh …` should work — it is the same jar and the same properties — but it has not
  been run, and it is a second comparison worth making.
