# TRAILS — Addendum 3: CQRS, Postgres Partitioning, and Scale Architecture

**Addendum to:** TRAILS-SPEC.md v2.0
**Date:** 2026-02-26
**Status:** Design notes

---

## The Problem

The original spec stores everything in a single Postgres instance. Over time:

- The `apps` table grows indefinitely — every task/device ever registered.
- The `messages`, `snapshots`, `audit_log` tables grow even faster.
- Read queries (auth checks, tree walks, status lookups) compete with writes (status updates, audit inserts).
- A hot device sending status every 10 seconds causes write pressure on the same rows that dashboards are reading.

At 50,000 devices sending status every minute, that's 50K writes/minute to `snapshots` plus 50K updates/minute to `apps.status`. Postgres can handle this, but not forever, and not while running recursive tree queries simultaneously.

## The Core Insight: Immutable vs Mutable Data

Most TRAILS data is **write-once, read-many:**

| Data | Written when | Updated? | Volume |
|---|---|---|---|
| `app_id`, `parent_id`, `pub_key` | Registration | **Never** | One row per task/device ever |
| `app_name`, `originator`, `role_refs`, `tags` | Registration | **Never** | Same row |
| `process_info` (pid, hostname, etc.) | Registration | **Never** | Same row |
| Messages (status, result, error) | When sent | **Never** | Append-only, high volume |
| Snapshots | When sent | **Never** | Append-only, high volume |
| Audit log | When action occurs | **Never** | Append-only |
| Crashes | When detected | **Never** | Append-only |
| Keypair grants | When admin creates | Rarely (revoke = delete) | Low volume |
| Role refs | When admin creates | Rarely | Low volume |
| `status` (connected/running/done/etc.) | State transitions | **Yes — only mutable field** | One field per task |

The vast majority of data is immutable once written. The only thing that changes per task is `status`. This is the classic CQRS split.

## The Architecture

```
                    ┌─────────────────────┐
                    │   trailsd cluster    │
                    │   (behind LB/VIP)   │
                    └──────────┬──────────┘
                               │
              ┌────────────────┼────────────────┐
              │                │                │
       ┌──────▼──────┐  ┌─────▼──────┐  ┌──────▼──────┐
       │  Write Path  │  │ Read Path  │  │  LRU Cache  │
       │              │  │            │  │  (in-proc)  │
       │ - register   │  │ - auth     │  │             │
       │ - status     │  │   checks   │  │ - pub_key   │
       │   update     │  │ - tree     │  │   lookups   │
       │ - messages   │  │   walks    │  │ - grant     │
       │ - snapshots  │  │ - snapshot │  │   checks    │
       │ - audit log  │  │   queries  │  │ - role_ref  │
       │ - crashes    │  │ - search   │  │   resolve   │
       │              │  │            │  │             │
       └──────┬───────┘  └─────┬──────┘  └─────────────┘
              │                │
              ▼                ▼
       ┌─────────────┐  ┌─────────────┐
       │  Postgres    │  │  Postgres   │
       │  Primary     │  │  Read       │
       │  (writes)    │──│  Replicas   │
       │              │  │  (reads)    │
       └─────────────┘  └─────────────┘
```

## Table Decomposition: Immutable vs Mutable

### The `apps` Table Splits Into Two

**`app_registry` — write-once, never updated:**

```sql
CREATE TABLE app_registry (
    app_id              UUID NOT NULL,
    parent_id           UUID,
    app_name            TEXT NOT NULL,
    pub_key             TEXT,
    namespace           TEXT,
    pod_name            TEXT,
    node_name           TEXT,
    pod_ip              INET,
    pid                 INTEGER,
    executable          TEXT,
    proc_uid            INTEGER,
    proc_gid            INTEGER,
    proc_user           TEXT,
    originator_sub      TEXT,
    originator_groups   TEXT[],
    role_refs           TEXT[],
    metadata_json       JSONB,
    start_deadline      INTEGER DEFAULT 300,
    registered_at       TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    start_day           SMALLINT NOT NULL,     -- days since 2025-01-01

    PRIMARY KEY (start_day, app_id)
) PARTITION BY RANGE (start_day);
```

This row is inserted once at registration and **never touched again.** Postgres knows it's cold data. No row locks, no HOT updates, no vacuum pressure. The table is effectively an append-only log.

**`app_status` — the only mutable state:**

```sql
CREATE TABLE app_status (
    app_id              UUID PRIMARY KEY,
    status              TEXT NOT NULL DEFAULT 'scheduled',
    last_snapshot_seq   BIGINT,
    connected_at        TIMESTAMPTZ,
    disconnected_at     TIMESTAMPTZ,
    server_instance     TEXT,
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
```

This is tiny — one row per active task/device, only the fields that actually change. Status transitions (`scheduled → connected → running → done`) update this table. The row is small (< 200 bytes), updated in-place, and can be hash-partitioned for parallel write throughput.

### Why This Split Matters for Postgres

When you `UPDATE` a row in Postgres, it doesn't modify in place — it creates a new row version (MVCC) and the old version needs vacuuming. If the `apps` table has 50 columns and you update `status` (1 column), Postgres still writes a full new row version with all 50 columns. Dead tuple bloat accumulates.

By splitting into `app_registry` (never updated) and `app_status` (frequently updated, tiny rows):

- `app_registry` generates **zero dead tuples.** No vacuum needed. Can be on slow storage.
- `app_status` generates dead tuples, but they're tiny (~200 bytes each). Vacuum is fast. Table stays small.
- Reads that only need immutable data (auth checks, tree walks) hit `app_registry` → read replicas. No write contention.
- Reads that need current status join `app_registry` with `app_status` on `app_id`. Both are indexed. Fast.

## Time-Based Partitioning: start_day

### The Encoding

```
start_day: SMALLINT (i16, but using as u16 conceptually)
    0     = 2025-01-01
    1     = 2025-01-02
    365   = 2026-01-01
    730   = 2027-01-01
    65535 = 2025 + (65535/365) ≈ year 2204

179 years of range. More than enough.
```

### Partition Layout

```sql
-- Create yearly partitions (or monthly for high-volume deployments)
CREATE TABLE app_registry_2025 PARTITION OF app_registry
    FOR VALUES FROM (0) TO (365);

CREATE TABLE app_registry_2026 PARTITION OF app_registry
    FOR VALUES FROM (365) TO (730);

CREATE TABLE app_registry_2027 PARTITION OF app_registry
    FOR VALUES FROM (730) TO (1096);

-- Same pattern for messages, snapshots, audit_log, crashes
CREATE TABLE messages (
    id              BIGSERIAL,
    app_id          UUID NOT NULL,
    start_day       SMALLINT NOT NULL,
    msg_type        TEXT NOT NULL,
    seq             BIGINT NOT NULL,
    payload_json    JSONB,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    PRIMARY KEY (start_day, id)
) PARTITION BY RANGE (start_day);

CREATE TABLE snapshots (
    id              BIGSERIAL,
    app_id          UUID NOT NULL,
    start_day       SMALLINT NOT NULL,
    snapshot_json   JSONB NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    PRIMARY KEY (start_day, id)
) PARTITION BY RANGE (start_day);

CREATE TABLE audit_log (
    id              BIGSERIAL,
    start_day       SMALLINT NOT NULL,
    timestamp       TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    action          TEXT NOT NULL,
    target_app_id   UUID,
    auth_domain     TEXT NOT NULL,
    oauth_subject   TEXT,
    source_ip       INET,

    PRIMARY KEY (start_day, id)
) PARTITION BY RANGE (start_day);
```

### Why start_day in the Wire Protocol

When a client sends a WebSocket message, it includes its `start_day`:

```json
{
  "type": "message",
  "app_id": "550e8400-...",
  "start_day": 421,
  "header": {"msg_type": "Status", "seq": 5},
  "payload": {"progress": 0.45},
  "sig": "..."
}
```

The `start_day` is set once at `trails_init()` (computed from `scheduledAt` in TRAILS_INFO). It never changes. The client sends it with every message as a **partition hint.**

trailsd uses this to:

1. Route reads directly to the correct partition (no partition scan)
2. Route writes directly to the correct partition
3. Build cache keys that include partition info

### Why This Matters at Scale

Without partitioning, a tree walk query:

```sql
WITH RECURSIVE tree AS (
    SELECT app_id, parent_id FROM app_registry WHERE app_id = :root
    UNION ALL
    SELECT a.app_id, a.parent_id
    FROM app_registry a JOIN tree t ON a.parent_id = t.app_id
)
SELECT * FROM tree;
```

Scans the entire `app_registry` table. With millions of rows accumulated over years, this is slow.

With partition pruning and a `start_day` range:

```sql
-- "Show me the tree for today's DAG run"
-- We know the root was created today (start_day = 421)
-- Children were also created today (same start_day)
-- Postgres prunes to just the 2026 partition

WITH RECURSIVE tree AS (
    SELECT app_id, parent_id FROM app_registry
    WHERE app_id = :root AND start_day = 421
    UNION ALL
    SELECT a.app_id, a.parent_id
    FROM app_registry a JOIN tree t ON a.parent_id = t.app_id
    WHERE a.start_day BETWEEN 420 AND 422  -- hint: children are near parent's day
)
SELECT * FROM tree;
```

Postgres scans only the relevant partition. Years of historical data are untouched.

## The LRU Cache Layer

### What Gets Cached

Most authorization checks follow the same pattern: "does this public key have access to this subtree?" The answer changes only when an admin modifies grants — which is rare.

```
Cache contents:
    pub_key → resolved grants (from keypair_grants + role_refs)
    app_id  → pub_key (from app_registry, immutable — never invalidates!)
    app_id  → parent_id (from app_registry, immutable — never invalidates!)
    role_ref name → resolved grants (from role_refs table)
    app_id  → current status (from app_status, short TTL)
```

The critical insight: **`app_id → pub_key` and `app_id → parent_id` never change.** These cache entries never need invalidation. They're valid forever. This is a direct consequence of the immutable registry design.

### Cache Hierarchy

```
Request arrives:
    │
    ├── L1: In-process LRU cache (per trailsd instance)
    │   Hit rate: ~95% for pub_key and parent_id lookups
    │   Latency: <1μs
    │
    ├── L2: Read replica Postgres
    │   For cache misses and complex queries (tree walks)
    │   Latency: 1–5ms
    │
    └── L3: Primary Postgres
        Only for writes (status updates, message inserts)
        Latency: 1–5ms
```

### Cache Invalidation Rules

| Cache entry | Invalidation trigger | Strategy |
|---|---|---|
| `app_id → pub_key` | Never (immutable) | Eternal, no eviction needed |
| `app_id → parent_id` | Never (immutable) | Eternal, no eviction needed |
| `role_ref → grants` | Admin updates role_ref | Broadcast invalidation across trailsd cluster |
| `pub_key → grants` | Admin modifies keypair_grants | Broadcast invalidation |
| `app_id → status` | Every status transition | Short TTL (5–30s) or event-based invalidation |

The first two rows — which are the most frequently accessed — **never need invalidation.** This gives the LRU cache an extraordinary effective hit rate because the working set of pub_keys and parent_ids for active devices fits easily in memory and never expires.

## The Request Flow

### Incoming Status Message (Write Path)

```
ESP32 sends: {"type": "message", "app_id": "...", "start_day": 421,
              "header": {"msg_type": "Status"}, "payload": {...}, "sig": "..."}

trailsd:
    1. LRU cache: lookup pub_key for app_id
       → HIT (immutable, cached forever): "ed25519:AAAA..."
    
    2. Verify Ed25519 signature against cached pub_key
       → Valid (microseconds, no I/O)
    
    3. Write path (primary Postgres):
       a. INSERT INTO snapshots (start_day=421, ...) — append-only, partitioned
       b. UPDATE app_status SET status='running', updated_at=now()
          WHERE app_id = ...  — tiny row, fast
       c. INSERT INTO messages (start_day=421, ...) — append-only, partitioned
    
    4. Internal event bus → Kafka/NATS (if fan-out enabled)
    
    Total: ~2–5ms, one cache hit + one Postgres round-trip
```

### Incoming Control Command (Read + Write Path)

```
Mom's phone: POST /api/v1/apps/$TV_UUID/control
             X-Trails-PubKey: ed25519:AAAA...

trailsd:
    1. LRU cache: lookup grants for pub_key "AAAA..."
       → HIT: [scope: $HOUSE_UUID, roles: [read, write, cancel], cascade: true]
    
    2. LRU cache: is $TV_UUID a descendant of $HOUSE_UUID?
       → LRU cache: parent_id chain walk:
         $TV_UUID → parent: $LIVING_ROOM_UUID (cached, immutable)
         $LIVING_ROOM_UUID → parent: $HOUSE_UUID (cached, immutable)
         Match! TV is descendant of house.
    
    3. Verify signature → valid
    
    4. Deliver command to TV via WebSocket
    
    5. Write path: INSERT INTO audit_log (start_day=421, ...)
    
    Total: ~1–3ms, all cache hits, one Postgres insert for audit
```

### Tree Query (Read Path)

```
trails tree --uuid $HOUSE_UUID

trailsd:
    1. Read replica Postgres:
       Recursive CTE on app_registry with start_day partition pruning
       JOIN app_status for current status
    
    2. No write path touched
    
    3. Cache populated as side-effect for subsequent queries
```

## Hash Partitioning for app_status

The `app_status` table is the only heavily-updated table. For very large deployments (100K+ concurrent devices), hash-partition it:

```sql
CREATE TABLE app_status (
    app_id      UUID NOT NULL,
    status      TEXT NOT NULL DEFAULT 'scheduled',
    last_snapshot_seq BIGINT,
    connected_at    TIMESTAMPTZ,
    disconnected_at TIMESTAMPTZ,
    server_instance TEXT,
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (app_id)
) PARTITION BY HASH (app_id);

CREATE TABLE app_status_p0 PARTITION OF app_status
    FOR VALUES WITH (modulus 16, remainder 0);
CREATE TABLE app_status_p1 PARTITION OF app_status
    FOR VALUES WITH (modulus 16, remainder 1);
-- ... through p15

-- 16 partitions: each handles ~6K devices out of 100K
-- UPDATE contention spread across partitions
-- Vacuum runs per-partition, fast
```

## Retention and Archival

Because data is time-partitioned by `start_day`, retention is trivial:

```sql
-- Drop all data older than 2 years (730 days)
DROP TABLE app_registry_2025;
DROP TABLE messages_2025;
DROP TABLE snapshots_2025;
DROP TABLE audit_log_2025;

-- Or for compliance: detach and move to cold storage
ALTER TABLE app_registry DETACH PARTITION app_registry_2025;
-- Move to S3/GCS via pg_dump, keep for auditors
```

No `DELETE FROM ... WHERE created_at < ...` running for hours with massive vacuum. One DDL statement drops or detaches an entire year. Instant.

## The trailsd Cluster

With CQRS and caching, trailsd scales horizontally:

```
Load Balancer / Service VIP
    │
    ├── trailsd-1 (LRU cache, WebSocket connections for devices A-M)
    ├── trailsd-2 (LRU cache, WebSocket connections for devices N-Z)
    ├── trailsd-3 (LRU cache, WebSocket connections for overflow)
    │
    ├── Postgres Primary (writes only)
    ├── Postgres Replica 1 (reads for trailsd-1)
    ├── Postgres Replica 2 (reads for trailsd-2)
    └── Postgres Replica 3 (reads for trailsd-3)
```

Each trailsd instance has its own LRU cache. Cache entries for immutable data (pub_key, parent_id) never need cross-instance invalidation. Cache entries for grants/role_refs are invalidated via broadcast (Postgres LISTEN/NOTIFY or Redis pub/sub for invalidation signals only).

Sticky sessions (by `app_id` hash) ensure a device's WebSocket always hits the same trailsd instance, maximizing cache hit rate.

## Summary of Data Paths

| Data | Mutability | Partitioned by | Storage tier | Accessed via |
|---|---|---|---|---|
| `app_registry` | Immutable | start_day (range) | Read replicas | LRU cache → Read replica |
| `app_status` | Mutable (status only) | app_id (hash) | Primary | LRU cache (short TTL) → Primary |
| `messages` | Append-only | start_day (range) | Read replicas | Read replica |
| `snapshots` | Append-only | start_day (range) | Read replicas | LRU cache (latest) → Read replica |
| `audit_log` | Append-only | start_day (range) | Primary (write) → Replicas (read) | Read replica |
| `keypair_grants` | Rarely modified | None (small table) | Primary | LRU cache → Read replica |
| `role_refs` | Rarely modified | None (small table) | Primary | LRU cache → Read replica |

## Spec Integration Notes

- §22 (Postgres Schema): Split `apps` into `app_registry` (immutable) + `app_status` (mutable). Add `start_day` column and partition definitions.
- §8 (Wire Protocol): Add `start_day` field to all client messages as partition hint.
- §5 (TRAILS_INFO): Client computes `start_day` from `scheduledAt` at init time.
- §21 (Server Architecture): Add CQRS read/write path separation, LRU cache layer, Postgres primary/replica topology.
- §27 (Overhead Analysis): Update with cache hit rates and query latencies.

---

*Addendum 3 introduces CQRS separation, time-based partitioning, and an LRU cache layer. The key insight: TRAILS data is overwhelmingly immutable (identity, keys, parentage, messages, snapshots) with only one mutable field (status). Separating these allows the immutable data to be cached eternally and queried from read replicas, while the tiny mutable state is updated on the primary with minimal write amplification. The `start_day` partition hint from the client enables partition pruning without server-side computation.*