# TRAILS: Tree-scoped Relay for Application Info, Lifecycle, and Signaling

## Complete Specification

**Personal Open-Source Project — GssMahadevan**
**License:** Apache 2.0 / MIT dual
**Status:** Design / Spec Phase
**Version:** 2.0 (evolved through design discussion)

---

## Table of Contents

1. [Problem Statement](#1-problem-statement)
2. [What TRAILS Is (and Is Not)](#2-what-trails-is-and-is-not)
3. [Core Design Principles](#3-core-design-principles)
4. [Architecture Overview](#4-architecture-overview)
5. [TRAILS_INFO Envelope](#5-trails_info-envelope)
6. [Identity Model](#6-identity-model)
7. [Two-Phase Lifecycle Protocol](#7-two-phase-lifecycle-protocol)
8. [Wire Protocol](#8-wire-protocol)
9. [Data Path (Child → Parent)](#9-data-path-child--parent)
10. [Control Path (Parent/Admin → Child)](#10-control-path-parentadmin--child)
11. [Open Command Vocabulary](#11-open-command-vocabulary)
12. [Tree-Wide Cascading Cancellation](#12-tree-wide-cascading-cancellation)
13. [Snapshots and State Reporting](#13-snapshots-and-state-reporting)
14. [Distributed waitpid](#14-distributed-waitpid)
15. [Security Model](#15-security-model)
16. [Authorization and RBAC](#16-authorization-and-rbac)
17. [Role References (K8s-Style)](#17-role-references-k8s-style)
18. [Multiple Identities Per Process](#18-multiple-identities-per-process)
19. [Daemonset Crash and Client Reconnection](#19-daemonset-crash-and-client-reconnection)
20. [Observer Model (Future)](#20-observer-model-future)
21. [Server Architecture](#21-server-architecture)
22. [Postgres Schema](#22-postgres-schema)
23. [REST API](#23-rest-api)
24. [Client Libraries (Native per Language)](#24-client-libraries-native-per-language)
25. [CLI Client (Bash/Terminal)](#25-cli-client-bashterminal)
26. [Repository Structure](#26-repository-structure)
27. [Overhead Analysis](#27-overhead-analysis)
28. [Comparison with Existing Systems](#28-comparison-with-existing-systems)
29. [Phase Plan](#29-phase-plan)

---

## 1. Problem Statement

Wherever a parent entity spawns a child — a K8s pod launching a worker, a VM starting a subprocess, a gateway provisioning an IoT device, a phone delegating to an appliance — a universal gap exists. There is no standard mechanism for **authenticated ancestor-based control** over a tree of entities: structured data exchange, bidirectional commands, and cascading lifecycle management across any infrastructure boundary.

The gap manifests in five ways:

1. **Lifecycle awareness** — did the child even start? The infrastructure layer may know (K8s pod phase, OS process table), but the parent entity doesn't get a structured signal tied to its own job ID. If a child never starts, the parent may not know for minutes. This is true for K8s orchestrators (7 known failure modes, up to 300s detection), for IoT gateways (device powered but firmware crashed), and for mobile apps (delegated task silently failed).

2. **Structured result delivery** — existing systems return an integer (Unix exit code, K8s exit status) or nothing at all (IoT device, mobile delegate). Neither carries business data. "The job succeeded" tells the parent nothing about what was produced. Teams work around this with shared filesystems, S3 paths, Redis, MQTT retained messages, or ad-hoc HTTP callbacks — each non-standard, non-portable, and coupled to a specific infrastructure.

3. **Structured error context** — an exit code or connection drop tells the parent nothing about why the child failed, at what point, on what data, or where to resume. Log parsing is fragile and non-standard across platforms.

4. **Bidirectional control** — there is no standard way for a parent to tell a running child: "pause", "change your batch size", "switch configuration", "enable debug logging." Unix has 31 signals with no payload. K8s has "delete the pod." IoT protocols have publish/subscribe but no acknowledged command/response. None support application-level commands with structured payloads and responses.

5. **Tree-wide cascading operations** — when an operator cancels a workflow, the root may die, but its children, grandchildren, and great-grandchildren across namespaces, clusters, VMs, and devices continue running as orphans. No existing system provides `pkill --tree` across network boundaries.

Every team reinvents solutions with ad-hoc HTTP callbacks, NATS sidecars, Redis pub/sub, MQTT topics, shared volumes, or polling observability platforms. None are standardized, portable, or zero-config for the application developer.

### Cooperative Model

TRAILS addresses this gap through **cooperative structured communication** following the parent-child model. It complements — not replaces — OS and infrastructure enforcement. Unix signals, K8s pod deletion, cgroup limits, and container runtime controls remain the mandatory enforcement layer. TRAILS provides the structured communication layer that sits alongside: structured results, bidirectional commands, and cascade coordination. For mandatory enforcement, TRAILS detection (e.g., `never_started`) triggers OS-level kill mechanisms.

---

## 2. What TRAILS Is (and Is Not)

### The Primitive

TRAILS provides **authenticated ancestor-based control over a tree of entities** — a universal, cross-platform, persistent, tree-scoped communication channel between parent and child, carrying structured lifecycle events, business data, and bidirectional control commands, with cascading operations across the entire entity tree regardless of infrastructure boundaries.

TRAILS answers three questions that no existing system answers together:

- **What did this entity produce?** (structured result, not exit code)
- **Why exactly did it fail?** (structured error context, not log grep)
- **Can I tell it to change behavior at runtime?** (open command vocabulary, not just SIGTERM)

Plus one capability that nothing provides:

- **Cancel an entire distributed entity tree with one command**, gracefully, bottom-up, across namespaces, clusters, VMs, and devices.

### Trust Model

TRAILS uses a simple point-to-point Ed25519 trust model for internal tree communication. This is not a replacement for X.509 — it operates alongside it, standing on the shoulders of giants.

**At the perimeter:** trailsd's external endpoint uses standard TLS with CA-issued certificates. This is X.509 doing what X.509 does best.

**Inside the perimeter:** Processes and devices trust trailsd's Ed25519 public key directly. Both sides hold each other's pub_key — mutual authentication without mTLS certificate overhead. This is designed for ephemeral, high-volume process lifecycles (thousands of processes with minutes/hours lifetime) where CA-issued per-process certificates are impractical. VPC-internal entities should not need external CA round-trips to communicate with their own parent.

**A unique property:** For transient processes, the child's Ed25519 private key exists only in RAM and dies with the process — no persistent key material to steal. Combined with infrastructure-independent 32-byte pub_key identity, this enables mutual-authenticated re-parenting as a routine state transition. The entire K8s/mTLS ecosystem has accepted workload re-parenting as impractical because certificate-bound identity anchors workloads to their issuing infrastructure. TRAILS' ephemeral key model sidesteps this constraint — not by design intent, but as a natural consequence of choices made for ephemeral processes and constrained devices.

### Transport Agnosticism

TRAILS is transport-agnostic. The `serverEp` URL scheme in TRAILS_INFO declares the transport:

`ws://`, `wss://`, `http://`, `https://`, `h3://`, `udp://`, `ble://`, `lora://`, `mqtt://`

trailsd implements transport adapters. The core protocol (message format, signing, tree semantics) is independent of transport. WebSocket is the reference transport for Phase 1 — not the only transport TRAILS supports. For constrained networks where no direct TRAILS transport is available, a gateway bridges devices into the tree.

### Phasing Discipline

The protocol is designed broadly to avoid breaking changes when new use cases are enabled in later phases. Fields like `start_day` (partition hint), `secLevel` (security tier), and `serverEp` (transport scheme) are in the wire format from v1 even though Phase 1 uses only a subset.

The implementation is phased: Phase 1 is K8s orchestration (the original use case). Subsequent phases expand to IoT device management, mobile multi-persona identity, embedded systems, and federated multi-cluster deployments. You cannot add tree-scoped auth later if no `parentId` from the start. You cannot support ESP32 later if the message format requires 100KB TLS state from the start.

### TRAILS Is Not

- **Not a replacement for OS enforcement** — Unix signals, K8s pod deletion, cgroup limits remain the mandatory enforcement layer. TRAILS is the cooperative communication layer alongside them.
- **Not a replacement for X.509 PKI** — TLS/X.509 secures the perimeter. TRAILS uses simple Ed25519 inside the perimeter for a different problem space (ephemeral, high-volume, VPC-internal).
- **Not a replacement for MQTT** — TRAILS offers an alternative for cases needing tree structure, cascade lifecycle, and structured results. Flat telemetry with no parent-child relationships is fine with MQTT.
- **Not a replacement for gRPC or service meshes** — microservices with Istio/Linkerd don't need TRAILS. TRAILS helps when entities form a tree with lifecycle dependencies.
- **Not a health monitor** — K8s probes handle liveness. Crash detection is a side effect of the lifecycle protocol, not the headline feature.
- **Not a logging system** — stdout/stderr plus Fluentd/Loki handle logs. TRAILS carries structured data, not log streams.
- **Not a metrics system** — Prometheus handles gauges and counters. TRAILS carries business results, not time-series metrics.
- **Not a general-purpose message bus** — communication follows the parent-child tree. Siblings cannot talk to each other except through their common parent. This is intentional for security.
- **Not needed for all workloads** — flat telemetry is fine with MQTT. Microservices are fine with gRPC + Istio. Batch jobs are fine with bare K8s. TRAILS helps when you have a tree of entities needing structured communication with ancestors — and the spec should be honest about where it adds value and where it doesn't.

---

## 3. Core Design Principles

1. **Data/Control path, not heartbeat path** — TRAILS carries business-meaningful messages. No empty pings. The WebSocket connection sits idle by default and activates only when someone has something meaningful to say. If no liveness system exists (non-K8s environments), an optional low-frequency keepalive (configurable, default 5 minutes) can be enabled as a fallback.

2. **Zero developer overhead** — two lines of code to integrate. The client library handles everything. If `TRAILS_INFO` is absent, `trails_init()` returns a no-op client. Developers can leave TRAILS calls in their code unconditionally.

3. **UUID identity** — every node in the tree is identified by a UUID v4. Globally unique, collision-free, permanent. Human-readable names and tags are annotations, never used as keys.

4. **Tree-scoped authorization** — a parent can command its children. A child sends data to its parent. No process can address another process it has no lineage relationship with, unless explicitly granted access via RBAC.

5. **Client is permission-unaware** — the client library contains zero security logic. The daemonset enforces all authorization. If a message reaches the client, it's already authorized.

6. **Star topology** — all communication routes through the TRAILS daemonset. No process-to-process direct connections. This simplifies security (one trust anchor), routing, and the client library.

7. **Postgres-backed** — durable state, survives server restart, queryable history, audit trail. Not Redis (volatile), not SQLite (not shared across replicas).

8. **Open command vocabulary** — the server doesn't interpret command actions or payloads. It's an opaque envelope. The meaning of commands is entirely between sender and receiver. Unknown commands warn and continue, never crash.

9. **Platform-agnostic** — the tree doesn't care whether a node is a K8s pod, a bare metal VM, a macOS laptop process, an Android app, or an iOS app. The only requirement is network reachability to the TRAILS server.

10. **Native clients per language** — each client library is written idiomatically in its target language. No FFI, no cross-compilation, no packaging nightmares. Protocol conformance tests ensure interoperability.

---

## 4. Architecture Overview

```
                        ┌─────────────────────────────────┐
                        │     TRAILS Daemonset (Rust)      │
                        │                                 │
                        │  ┌───────────┐  ┌────────────┐  │
                        │  │ WebSocket │  │ Postgres   │  │
                        │  │ Handler   │──│ State Store│  │
                        │  └─────┬─────┘  └────────────┘  │
                        │        │                        │
                        │  ┌─────┴──────┐  ┌───────────┐  │
                        │  │ REST API   │  │ Auth/RBAC │  │
                        │  │ (external  │  │ Enforcer  │  │
                        │  │  actors)   │  │           │  │
                        │  └────────────┘  └───────────┘  │
                        └────────┬────────────────────────┘
                                 │
              ┌──────────────────┼──────────────────┐
              │                  │                  │
         ┌────┴────┐       ┌────┴────┐       ┌────┴────┐
         │ K8s Pod │       │ Bare    │       │ Android │
         │ ns:data │       │ Metal   │       │ Device  │
         │         │       │ VM      │       │         │
         │ Python  │       │ Go svc  │       │ Kotlin  │
         │ client  │       │ client  │       │ client  │
         └─────────┘       └─────────┘       └─────────┘

    Star topology: every process talks ONLY to the daemonset.
    Daemonset routes messages based on parent-child tree.
    Daemonset enforces all authorization.
```

---

## 5. TRAILS_INFO Envelope

Set by whoever creates the child process (orchestrator, Helm chart, kubectl, bash script, etc.) as an environment variable containing base64-encoded JSON.

### Format

```bash
TRAILS_INFO=eyJ2IjoxLCJhcHBJZCI6IjU1MGU4NDAwLS4uLi...
```

### Decoded JSON

```json
{
  "v":             1,
  "appId":         "550e8400-e29b-41d4-a716-446655440000",
  "parentId":      "440e7300-d18a-30c3-b605-335544330000",
  "appName":       "pii-scan-step1",
  "serverEp":      "wss://trails.trails-system.svc:8443/ws",
  "serverPubKey":  "ed25519:K2dG...",
  "secLevel":      "signed",
  "scheduledAt":   1740000000000,
  "startDeadline": 300,
  "originator": {
    "sub":    "gssmahadevan@company.com",
    "groups": ["data-eng"]
  },
  "roleRefs":      ["sre-on-call", "team-readonly"],
  "tags": {
    "dagRun": "daily-2025-02-25",
    "step":   "3"
  }
}
```

### Field Definitions

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `v` | integer | Yes | Protocol version. Currently 1. |
| `appId` | UUID v4 | Yes | Globally unique identifier for this task. Assigned by parent. |
| `parentId` | UUID v4 | Yes* | Parent's appId. Null only for root orchestrator. |
| `appName` | string | Yes | Human-readable name. Not unique — used for display only. |
| `serverEp` | string | Yes | TRAILS server WebSocket endpoint URL. |
| `serverPubKey` | string | Conditional | Server's Ed25519 public key. Required when `secLevel` is "signed" or "full". |
| `secLevel` | string | Yes | Security level: "open", "signed", or "full". |
| `scheduledAt` | integer | Yes | Epoch milliseconds when parent created this config. |
| `startDeadline` | integer | No | Seconds within which child must call `trails_init()`. Default 300. |
| `originator` | object | No | Identity of the human who initiated the root of the tree. Inherited from root to all descendants. |
| `originator.sub` | string | No | OAuth/OIDC subject (e.g., email) of the originating human. |
| `originator.groups` | string[] | No | Group memberships of the originator. |
| `roleRefs` | string[] | No | Array of role reference names for external access control. See §17. |
| `tags` | object | No | Arbitrary key-value pairs for grouping/filtering. Stored in Postgres as JSONB. |

### Design Rationale

- **Single env var** — doesn't pollute CLI args or require arg parsing changes.
- **Base64** — safe for any env var value, no quoting issues.
- **JSON** — extensible, add fields later without breaking existing clients.
- **UUID appId** — unique correlation key, set by orchestrator, used to track this specific task instance.
- **`v` field** — enables protocol evolution without breaking existing clients.

### Non-Env-Var Delivery

On platforms where environment variables are awkward (mobile apps, embedded devices), the parent can pass the config any way it wants — Android intent extras, iOS launch arguments, MQTT payload, command-line JSON — and the child calls `trails_init_with(config)` instead of `trails_init()`.

---

## 6. Identity Model

### Three Layers of Identity

Every action in TRAILS involves identity at up to three layers:

**Layer 1 — Process Identity (always, automatic):**

Collected automatically at `trails_init()` time.

```
pid, uid, gid, hostname, namespace, pod_ip, node_name, executable
```

This tells you *what is running*. Available on every OS. K8s-specific fields (namespace, pod_ip, node_name) are empty on non-K8s platforms, replaced by OS equivalents (machine name, LAN IP).

**Layer 2 — Cryptographic Identity (always, automatic):**

```
Ed25519 keypair — generated per process at trails_init()
Private key: lives only in process memory, dies with process
Public key: sent during registration, stored in Postgres
```

This proves *you are the process you claim to be*. Prevents impersonation.

**Layer 3 — Human Identity (external commands only):**

```
OAuth/OIDC subject, groups, claims
e.g., "alice@company.com", groups: ["sre-team"]
```

This tells you *which person in the organization is acting*. Required for any command from outside the tree (REST API calls from humans or service accounts). Not needed for tree-internal communication.

### UUID Is Identity

Every `appId` and `parentId` MUST be UUID v4 — globally unique, no collision possible, no ambiguity across clusters, across time, across anything.

- **UUID is the address.** Every API, protocol message, Postgres foreign key, and cascade operation uses UUID.
- **appName and tags are labels.** Human-readable, searchable, but never used as lookup keys.
- Two tasks can have `appName: "extract"` — that's fine. Their `appId` is never the same.

### Originator Trace

The `originator` field tracks the human who started the root of the tree. It is inherited unchanged from the root node to every descendant through the `TRAILS_INFO` envelope.

Every node in the tree carries the originator's identity, even when the immediate actor is a service account. This answers the audit question: "which human caused all of this to happen?"

### Actor vs Process vs Originator in Postgres

```
apps table:
    proc_uid / proc_gid / proc_user     → what process is running (different at every level)
    originator_sub / originator_groups   → who started the root (same throughout tree)
```

---

## 7. Two-Phase Lifecycle Protocol

### The Gap TRAILS Fills

Between "parent decides to create a child" and "child is running and communicating," there is a window where failures are invisible. K8s knows the pod is Pending or CrashLoopBackOff, but the orchestrator doesn't get a structured signal tied to its job ID.

### Phase A — Parent Declares Intent

Before creating the pod/process, the parent registers intent with TRAILS:

```
POST /api/v1/children
{
  "parentId":      "aaa-...",
  "appId":         "bbb-...",
  "appName":       "pii-scan-step1",
  "startDeadline": 300,
  "roleRefs":      ["sre-on-call"],
  "tags":          {"dagRun": "daily-2025-02-25"}
}

Server creates row: status = 'scheduled', records timestamp
```

### Phase B — Child Connects (or Doesn't)

If the child starts and calls `trails_init()`, status transitions:

```
scheduled → connected → running → done | error
```

If the child never connects within `startDeadline`:

```
scheduled → start_failed (crash_type = 'never_started')
```

### State Machine

```
scheduled ──────► connected ──────► running ──────► done
    │                 │                │               
    │                 │                ├──────► error  
    │                 │                │               
    │                 │                ├──────► crashed
    │                 │                │               
    │                 │                └──────► cancelled
    │                 │
    │                 └──────► crashed (connection drop)
    │
    └──────► start_failed (deadline expired, child never called trails_init)

Additional transient states:
    reconnecting  — daemonset restarted, waiting for client to reconnect
    lost_contact  — client didn't reconnect within window after daemonset restart
```

### Crash Types

| crash_type | Signal | Detection time |
|---|---|---|
| `connection_drop` | WebSocket TCP close | 0–2 seconds |
| `heartbeat_timeout` | No keepalive (if enabled) | Configurable (default 5 min) |
| `never_started` | Child never called `trails_init()` | `startDeadline` (default 300s) |

### Ordering

1. Parent generates child UUID v4
2. Parent calls `POST /api/v1/children` → server expects this child
3. Parent creates pod/process with `TRAILS_INFO` containing that UUID
4. Child starts (or doesn't)

If step 3 fails (K8s API error), the parent can call `DELETE /api/v1/children/{appId}` or let it time out.

---

## 8. Wire Protocol

### Transport

WebSocket (RFC 6455). One persistent connection per client process to the TRAILS server. The connection sits idle by default — no mandatory heartbeats. Messages flow only when there is business-meaningful data or control to communicate.

When multiple TRAILS identities share a process (see §18), they share a single WebSocket connection, multiplexed by `appId`.

### Client → Server Messages

**Registration (first message after WebSocket connect):**

```json
{
  "type": "register",
  "app_id": "550e8400-e29b-41d4-a716-446655440000",
  "parent_id": "440e7300-d18a-30c3-b605-335544330000",
  "app_name": "pii-scan-step1",
  "child_pub_key": "ed25519:base64...",
  "process_info": {
    "pid": 1,
    "ppid": 0,
    "uid": 1000,
    "gid": 1000,
    "hostname": "pii-scan-pod-xyz",
    "node_name": "aks-nodepool1-12345",
    "pod_ip": "10.244.1.15",
    "namespace": "datascan",
    "start_time": 1740000000000,
    "executable": "/usr/bin/python3"
  },
  "role_refs": ["sre-on-call", "team-readonly"],
  "sig": "ed25519:base64..."
}
```

**Re-Registration (after daemonset restart):**

```json
{
  "type": "re_register",
  "app_id": "550e8400-e29b-41d4-a716-446655440000",
  "last_seq": 47,
  "pub_key": "ed25519:base64...",
  "sig": "ed25519:base64..."
}
```

**Data messages (status, result, error):**

```json
{
  "type": "message",
  "app_id": "550e8400-...",
  "header": {
    "msg_type": "Status",
    "timestamp": 1740000060000,
    "seq": 5,
    "correlation_id": null
  },
  "payload": {
    "phase": "processing",
    "progress": 0.45,
    "rows_processed": 50000,
    "checkpoint": "customers:row:50000"
  },
  "sig": "ed25519:base64..."
}
```

**Message types (app → server):**

| msg_type | Purpose |
|----------|---------|
| `Status` | Progress update, current state, snapshot |
| `Result` | Business result (job output). Typically at end of job. |
| `Error` | Structured error report with context. |

**Graceful disconnect:**

```json
{
  "type": "disconnect",
  "app_id": "550e8400-...",
  "reason": "completed"
}
```

### Server → Client Messages

**Ack:**

```json
{
  "type": "ack",
  "seq": 5
}
```

**Control command:**

```json
{
  "type": "control",
  "action": "cancel",
  "correlation_id": "ctrl-001",
  "payload": {
    "reason": "user requested cancellation",
    "cascade": true,
    "initiated_by": "alice@company.com"
  },
  "sig": "ed25519:base64..."
}
```

### Signing

When `secLevel` is "signed" or "full":

- All client→server messages are signed with the client's Ed25519 private key. The server verifies against the registered public key.
- All server→client control messages are signed with the server's Ed25519 private key. The client verifies against the `serverPubKey` from `TRAILS_INFO`.
- The signature covers the canonical JSON encoding of `header` + `payload` (or `action` + `payload` for control messages).

### Security Tiers

| Tier | Transport | Signing | Use case |
|---|---|---|---|
| `open` | `ws://` (plain) | None | Dev, local minikube, trusted network |
| `signed` | `ws://` (plain) | Ed25519 per message | Multi-tenant, network is internal — authenticity without encryption |
| `full` | `wss://` (TLS) | Ed25519 per message | Regulated environments — eavesdrop protection + authenticity |

---

## 9. Data Path (Child → Parent)

The primary data path is child → server (Postgres) → parent queries via REST.

### Status Updates

Sent at meaningful business points or fixed intervals. Each is a snapshot of the task's current state.

```python
g.status({"phase": "processing", "progress": 0.45, "table": "customers",
          "rows_done": 45000, "rows_total": 100000, "checkpoint": "customers:row:45000"})
```

### Business Results

Sent at completion. Structured JSON carrying the actual output of the job.

```python
g.result({"rows_scanned": 100000, "pii_columns_found": 4,
          "pii_detections": [{"column": "ssn", "type": "SSN", "count": 15000}]})
```

### Structured Errors

Sent on failure. Carries the full context of what went wrong.

```python
g.error("OutOfMemoryError processing table customers",
        detail={"row_number": 50231, "batch": 7, "memory_used_mb": 3800,
                "checkpoint": "customers:row:50000"})
```

### Parent Retrieves Data

Via REST API:

```
GET /api/v1/apps/{appId}/messages?type=Result
GET /api/v1/apps/{appId}/messages?type=Error
GET /api/v1/apps/{appId}/snapshots/latest
```

Or via watch WebSocket for real-time streaming:

```
WS /api/v1/watch?app_id={appId}
```

---

## 10. Control Path (Parent/Admin → Child)

### Tree-Internal Control (Parent → Child)

The parent sends commands via REST. The server routes to the child's WebSocket.

```
POST /api/v1/apps/{childAppId}/control
{
  "action": "cancel",
  "payload": {"reason": "user requested"}
}
```

Authorization: the server verifies that the caller is the parent (or ancestor) of the target. No configuration needed — the parent-child relationship is the authorization.

### External Control (Human/Service → Child)

External actors (humans, monitoring services) send commands via REST with OAuth bearer token.

```
POST /api/v1/apps/{appId}/control
Authorization: Bearer eyJ...
{
  "action": "reconfig",
  "payload": {"batch_size": 500}
}
```

Authorization: the server validates the OAuth token, resolves role refs, checks grants. See §16.

### Command Response

Every command gets an acknowledged response:

```json
{"ack": true, "result": {"applied": true, "new_batch_size": 500}}
```

or:

```json
{"ack": true, "result": {"applied": false, "reason": "min batch is 1000"}}
```

or for unknown commands:

```json
{"ack": false, "error": "unknown_action", "action": "foo"}
```

---

## 11. Open Command Vocabulary

The control message `action` and `payload` are opaque to the server. The server routes and logs them. What they mean is defined by the application.

### The Contrast with Unix/K8s

Unix: 31 signals, no payload, no acknowledgment, no response.
K8s: "delete pod" — one verb, no application semantics.
TRAILS: any string action, any JSON payload, acknowledged response, persistent audit trail.

### Example Command Categories

**Lifecycle:**

```json
{"action": "cancel",  "payload": {"reason": "user requested"}}
{"action": "pause",   "payload": {"resume_after": "2025-02-25T10:00:00Z"}}
{"action": "resume"}
{"action": "drain",   "payload": {"finish_current_batch": true}}
```

**Configuration:**

```json
{"action": "reconfig", "payload": {"batch_size": 500}}
{"action": "reconfig", "payload": {"log_level": "debug"}}
{"action": "reconfig", "payload": {"model_version": "v2.3", "hot_swap": true}}
```

**Operational:**

```json
{"action": "checkpoint",   "payload": {"reason": "pre-maintenance"}}
{"action": "dump_state",   "payload": {"include_buffers": true}}
{"action": "throttle",     "payload": {"rate": 100, "unit": "rows/sec"}}
{"action": "enable_trace", "payload": {"sample_rate": 0.01, "duration_sec": 60}}
```

**Domain-specific:**

```json
{"action": "skip_partition",  "payload": {"partition": "2024-Q1"}}
{"action": "switch_model",    "payload": {"from": "gpt-4", "to": "claude-sonnet"}}
```

### Client-Side Handler Registration

```python
g = TrailsClient.init()

@g.on("reconfig")
def handle_reconfig(payload):
    if "batch_size" in payload:
        engine.set_batch_size(payload["batch_size"])
        return {"applied": True, "new_batch_size": payload["batch_size"]}
    return {"applied": False, "reason": "unknown config key"}

@g.on("checkpoint")
def handle_checkpoint(payload):
    offset = engine.save_checkpoint()
    return {"checkpoint": offset, "rows_so_far": engine.count}

# Unknown actions automatically:
#   1. Log warning: "Unknown action 'foo' received, ignoring"
#   2. Send response: {"error": "unknown_action", "action": "foo"}
#   3. Continue running — never crash on unknown command
```

### Capability Advertisement (Future)

Optionally, the registration message can include a manifest of supported commands:

```json
{
  "type": "register",
  ...
  "capabilities": ["cancel", "pause", "reconfig", "checkpoint", "dump_state"]
}
```

Stored in Postgres, queryable via REST. Allows dashboards and parents to know what commands a child understands before sending.

---

## 12. Tree-Wide Cascading Cancellation

### The Problem

An operator cancels a DAG run. The master task dies. But its 15 children across 8 nodes, some with their own sub-tasks, continue running. K8s doesn't know these pods are related. Airflow doesn't know about grandchildren. The operator manually hunts down orphans.

This is the distributed equivalent of the Unix orphan process problem, but worse because processes span machines, namespaces, clusters, VMs, and devices.

### The Solution

```
POST /api/v1/apps/{rootId}/cancel?cascade=true
```

The server resolves the full subtree from Postgres, then sends cancel bottom-up (leaves first, root last):

```sql
WITH RECURSIVE tree AS (
    SELECT app_id FROM apps WHERE app_id = :rootId
    UNION ALL
    SELECT a.app_id FROM apps a
    JOIN tree t ON a.parent_id = t.app_id
    WHERE a.status IN ('connected', 'running')
)
```

### Bottom-Up Ordering

Children clean up before parents, like `defer`/`finally` unwinding a stack:

```
        Root (DAG run)
        ├── Step A
        │   ├── Spark Driver
        │   │   ├── Executor 1  ← cancel first
        │   │   └── Executor 2  ← cancel first
        │   └── Sidecar         ← cancel second
        └── Step B
            └── GPU Job         ← cancel first

Cancel order: Executors → Sidecar/GPU Job → Spark Driver → Step A/B → Root
```

Each level waits for its children to acknowledge (or timeout) before proceeding to the next level up.

### Grace Period

Each node has a grace period. If the `on_cancel` hook doesn't return within the grace period, the server marks the node `force_killed` and moves on. A hung child doesn't block the entire cascade.

```
Total grace = max_depth × per_level_grace
4 levels deep × 30s grace = 120s maximum cancellation time
```

### Client-Side Cancel Hook

```python
g = TrailsClient.init()

def on_cancel(ctx):
    save_checkpoint()
    flush_buffers()
    # process exits after this returns

g.on_cancel(grace_seconds=30, hook=on_cancel)
```

```rust
impl TrailsClient {
    pub fn on_cancel<F>(&self, grace: Duration, hook: F)
    where F: FnOnce(CancelContext) + Send + 'static
    { todo!() }
}

pub struct CancelContext {
    pub reason: String,
    pub initiated_by: String,     // app_id or OAuth subject of who started the cancel
    pub grace_remaining: Duration,
}
```

### Cross-Boundary Reach

The cancel reaches any process that called `trails_init()`, regardless of where it runs:

| Mechanism | K8s same NS | K8s cross NS | Bare metal VM | Mobile device | Remote cloud |
|---|---|---|---|---|---|
| `kubectl delete ns` | ✓ | ✗ | ✗ | ✗ | ✗ |
| K8s owner GC | ✓ (1 level) | ✗ | ✗ | ✗ | ✗ |
| `kill -TERM -pgid` | local only | ✗ | local only | ✗ | ✗ |
| Airflow cancel | its own tasks | ✗ | ✗ | ✗ | ✗ |
| **TRAILS cascade** | **✓** | **✓** | **✓** | **✓** | **✓** |

---

## 13. Snapshots and State Reporting

### Concept

Children send periodic snapshots — either at fixed time intervals or at meaningful business milestones. Each snapshot is a full picture of the task's state at that moment.

```python
g.status({
    "state": "processing",
    "progress": 0.75,
    "table": "transactions",
    "rows_done": 75000,
    "rows_total": 100000,
    "memory_used_mb": 2100,
    "checkpoint": "transactions:row:50000"
})
```

### What This Enables

**Resume from checkpoint:** If the child crashes, the parent queries the last snapshot and resumes from the recorded checkpoint. No ad-hoc checkpoint files on shared volumes.

**Informed cancellation:** When the operator clicks cancel, the dashboard shows exactly what each task has done so far. The operator makes an informed decision, not a blind termination.

**Historical analysis:** Reconstruct the full timeline of any job from snapshot history. Memory growth, progress curves, performance trends.

### Snapshot Retention

- **Latest snapshot per app** — always kept
- **Result and Error messages** — always kept
- **Intermediate snapshots** — configurable retention (last N per app, or time-based)

---

## 14. Distributed waitpid

### Concept

`trails wait` is `waitpid()` for distributed systems:

```bash
$ trails wait --uuid 550e8400-e29b-41d4-a716-446655440000
{"rows": 100000, "pii_cols": 4, "duration_sec": 342}
$ echo $?
0
```

### Comparison

| Aspect | Unix `waitpid()` | `trails wait` |
|---|---|---|
| Addressing | PID (local, recycled) | UUID (global, permanent) |
| Scope | My children, this machine | Any task, any machine (with privilege) |
| Returns | Integer exit status | Structured JSON result |
| Multiple waiters | One process only | Any number of concurrent waiters |
| History | Gone after wait | Permanent in Postgres |

### Wait Variants

```bash
# Wait for specific task
trails wait --uuid $CHILD_ID

# Wait for any child of a parent to complete
trails wait --parent $PARENT_ID --any

# Wait for ALL children
trails wait --parent $PARENT_ID --all

# Wait with timeout
trails wait --uuid $CHILD_ID --timeout 3600

# Wait with progress streaming
trails wait --uuid $CHILD_ID --progress

# Wait for multiple specific tasks
trails wait --uuids $ID1,$ID2,$ID3 --any

# Non-blocking status check
trails status --uuid $CHILD_ID
```

### Exit Codes

| Code | Meaning |
|------|---------|
| 0 | Task succeeded |
| 1 | Task failed |
| 2 | Task crashed |
| 3 | Timeout |
| 4 | Task cancelled |

### Implementation

`trails wait` uses the watch WebSocket, not polling:

```
Connect to WS /api/v1/watch?app_id={uuid}
← receive status events (printed to stderr with --progress)
← receive terminal event → print result JSON to stdout, exit
```

If the task is already in a terminal state, the server immediately sends the terminal event. No race condition.

---

## 15. Security Model

### Star Topology Trust Model

```
        Parent A          Parent B
            \               /
             \             /
              ▼           ▼
         ┌─────────────────────┐
         │   TRAILS Daemonset   │    ← the ONLY hub, single trust anchor
         └─────────┬───────────┘
              ╱    │    ╲
             ╱     │     ╲
            ▼      ▼      ▼
        Child 1  Child 2  Child 3
```

No process talks directly to another process. Every message routes through the daemonset. The only trust relationship each process needs: "I trust this TRAILS server."

### Key Exchange During Registration

1. Parent creates `TRAILS_INFO` with `serverPubKey` included
2. Child generates ephemeral Ed25519 keypair at `trails_init()`
3. Child sends `child_pub_key` in registration message
4. Server stores child's public key in `apps` table
5. Future client→server messages: verified against child's pubkey
6. Future server→client messages: signed with server's private key, child verifies against `serverPubKey`

### Key Properties

- **Server keypair:** long-lived, generated at daemonset startup, stored in K8s Secret, survives restarts. Rotatable with dual-signature transition period.
- **Client keypair:** ephemeral, generated fresh at each `trails_init()`, lives only in process memory. No key storage, no key management.

### Two Authorization Domains

**Domain 1 — Tree-internal (process ↔ process):**

Authentication: Ed25519 signatures.
Authorization: parentId chain. Automatic. No configuration needed.

**Domain 2 — External (human/service → process):**

Authentication: OAuth/OIDC JWT (Bearer token on REST API).
Authorization: Role refs and grants. See §16 and §17.

---

## 16. Authorization and RBAC

### Three Phases of Authorization

**Phase 1 — Unix Permissions (initial implementation):**

- A process can control its descendants (parentId chain).
- Same UID can query/wait on matching tasks.
- Root/admin flag: unrestricted.

**Phase 2 — Group-Based RBAC:**

- Group memberships determine access (from Unix groups or K8s RBAC bindings).
- Role refs map groups to permissions. See §17.

**Phase 3 — OAuth/OIDC Integration:**

- OAuth token validates human identity against IdP.
- Claims (email, groups, roles) map to RBAC rules.
- `originator.authRef` field (null in Phase 1) carries OIDC reference.

### Role Vocabulary

Two base roles cover most cases:

| Role | Permissions |
|------|------------|
| `read` | Query status, snapshots, results, wait, view tree |
| `write` | Send control commands (reconfig, pause, checkpoint, cancel, etc.) |

Extended roles for organizations that need finer granularity:

| Role | Permissions |
|------|------------|
| `control` | Send control commands (reconfig, pause, checkpoint) |
| `cancel` | Cancel this task (and optionally cascade) |
| `admin` | All of the above + modify grants |

### Complete Permission Matrix

| Action | Tree-scoped (process) | External (human/service) |
|---|---|---|
| Send status/result | Ed25519 sig, registered appId | N/A |
| Cancel own children | parentId chain | N/A |
| Cancel others' tasks | Not allowed | OAuth + `write` or `cancel` role |
| Send control to child | parentId chain | OAuth + `write` or `control` role |
| Query own tree | parentId chain | N/A |
| Query others' tasks | Not allowed | OAuth + `read` role |
| Wait on own children | parentId chain | N/A |
| Wait on others' tasks | Not allowed | OAuth + `read` role |
| Admin operations | Never | OAuth + `admin` role |

### Long-Lived JWT Mitigation

| Strategy | Description |
|----------|-------------|
| Short-lived tokens | Access token: 15 min, refresh token: 8 hours |
| Server-side max age | Refuse tokens older than configurable limit regardless of JWT `exp` |
| Grant expiry | Grants themselves have `expiresAt` timestamps |
| Session tokens | TRAILS issues its own short-lived, scope-bound session tokens wrapping OAuth identity |
| Cron cleanup | Periodic job revokes expired sessions and grants |

### Audit Trail

Every external action is logged:

```sql
CREATE TABLE audit_log (
    id              BIGSERIAL PRIMARY KEY,
    timestamp       TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    action          TEXT NOT NULL,
    target_app_id   UUID,
    cascade         BOOLEAN DEFAULT false,
    payload_json    JSONB,
    auth_domain     TEXT NOT NULL,         -- "tree" or "external"
    source_app_id   UUID,                  -- if tree-internal
    oauth_subject   TEXT,                  -- if external: "alice@company.com"
    oauth_issuer    TEXT,
    oauth_groups    TEXT[],
    source_ip       INET,
    user_agent      TEXT
);
```

---

## 17. Role References (K8s-Style)

### Concept

Borrowing from K8s ClusterRole/RoleBinding: define reusable permission templates (role refs) once as an admin, reference them by name in child creation. The client code never contains permission logic.

### Admin Defines Role Refs

```json
POST /api/v1/admin/role-refs
{
  "name": "sre-on-call",
  "description": "SRE on-call can observe and intervene",
  "grants": [
    {"sub": "group:sre-team", "roles": ["read", "write", "cancel"]}
  ]
}
```

```json
{
  "name": "team-readonly",
  "description": "Team members can observe their own namespace",
  "grants": [
    {"sub": "group:data-eng", "roles": ["read"]},
    {"sub": "group:ml-eng", "roles": ["read"]}
  ]
}
```

```json
{
  "name": "monitoring",
  "description": "Monitoring services read-only",
  "grants": [
    {"sub": "sa:grafana-agent", "roles": ["read"]},
    {"sub": "sa:pagerduty-webhook", "roles": ["read"]}
  ]
}
```

### Children Reference by Name

In `TRAILS_INFO`:

```json
"roleRefs": ["sre-on-call", "team-readonly", "monitoring"]
```

Three short strings instead of verbose inline grant arrays.

### Resolution

The server resolves references at authorization check time:

```
"roleRefs": ["sre-on-call", "team-readonly", "monitoring"]

→ Union of all unique grants:
    group:sre-team         → [read, write, cancel]
    group:data-eng         → [read]
    group:ml-eng           → [read]
    sa:grafana-agent       → [read]
    sa:pagerduty-webhook   → [read]
```

If a subject appears in multiple refs with different roles, union applies.

### Graceful Degradation

| Scenario | Parent access | External access | Server action |
|---|---|---|---|
| All refs valid | Full (always) | Per resolved grants | Normal |
| Some refs invalid | Full (always) | Partial (valid refs only) | Warn in logs |
| All refs invalid | Full (always) | Admin only | Warn in logs |
| roleRefs empty/absent | Full (always) | Admin only | Safe default |

**A missing role ref never prevents the child from starting or communicating with its parent.** It only affects external access. The data/control path between parent and child is unconditional.

### Inheritance

By default, children inherit parent's roleRefs. Children can extend but not escalate:

```
Parent: roleRefs: ["sre-on-call", "monitoring"]
Child:  roleRefs: ["sre-on-call", "monitoring", "compliance-audit"]  (extended)
```

### Mixed: References Plus Inline Grants

For one-off grants that don't fit any template:

```json
{
  "roleRefs": ["sre-on-call", "monitoring"],
  "grants": [
    {"sub": "bob@company.com", "roles": ["read"], "expiresAt": "2025-02-26T00:00:00Z"}
  ]
}
```

Resolution: union of all roleRef grants plus any inline grants.

### Caching

Because role refs are named, stable, and rarely change, the server caches resolved permissions aggressively:

```
Cache key: role_ref name → resolved grants
Invalidation: only when admin updates the role_ref
TTL: hours (role definitions are stable)
```

Routine authorization checks hit cache, not Postgres.

### Layered Responsibility

| Layer | Who | Does what |
|-------|-----|-----------|
| Application | Developer | Business logic. `g.status()`, `g.result()`. **Never thinks about permissions.** |
| Orchestration | Platform team | Sets `roleRefs` in child creation templates. |
| Policy | Admin / Security | Defines role_refs in Postgres. Maps groups to permissions. |
| Enforcement | TRAILS Daemonset | Resolves, caches, checks, logs. Fully automatic. |

---

## 18. Multiple Identities Per Process

### The Problem

Not all parent-child relationships are between separate processes. Airflow runs multiple tasks within a single Python worker process. All share the same PID, hostname, and pod.

### The Solution

The client library supports multiple `TrailsClient` instances per process, each with its own `appId`:

```python
# Airflow worker process — single PID

dag_id = uuid.uuid4()
dag = TrailsClient.init_with(TrailsConfig(
    app_id=str(dag_id), parent_id=None, app_name="daily-etl", ...))

extract_id = uuid.uuid4()
dag.create_child(app_id=str(extract_id), name="extract")
extract = TrailsClient.init_with(TrailsConfig(
    app_id=str(extract_id), parent_id=str(dag_id), app_name="extract", ...))

extract.status({"rows_read": 50000})
extract.result({"rows": 100000})
extract.shutdown()   # marks extract as 'done', doesn't kill the process
```

### WebSocket Multiplexing

Multiple identities share a single WebSocket connection. Messages are multiplexed by `appId`:

```
Single WebSocket from PID 12345:
  → {"app_id": "dag-run-001", "type": "message", ...}
  → {"app_id": "task-ext-002", "type": "message", ...}
  ← {"app_id": "task-load-004", "type": "control", ...}
```

PROTOCOL.md specifies:

> A single WebSocket connection MAY carry messages for multiple app_ids. Each message MUST include its app_id. The server routes control messages to the correct handler regardless of physical connection.

### What Postgres Shows

```
app_id          parent_id       app_name      pid    hostname        status
dag-run-001     NULL            daily-etl     12345  airflow-pod-x   done
task-ext-002    dag-run-001     extract       12345  airflow-pod-x   done
task-tfm-003    dag-run-001     transform     12345  airflow-pod-x   done
```

Same PID, same hostname — but correct tree structure. The tree is logical, not physical.

### Cascade Cancel with Mixed Topology

Cancellation works identically for in-process tasks and external processes. For in-process tasks, the cancel message arrives on the shared WebSocket and is routed to the correct `on_cancel` handler.

---

## 19. Daemonset Crash and Client Reconnection

### Scenario

The daemonset will eventually crash — OOM kill, node reboot, rolling update. When it comes back, every client on that node has a dead WebSocket.

### Client Has Everything in Memory

- `TRAILS_INFO` (parentId, appId, serverEp, appName)
- Its own Ed25519 keypair (generated at `trails_init()`, still in memory)
- Its sequence counter

### Reconnection Protocol

```
Client detects connection lost (read/write error)
    │
    ├── Exponential backoff with jitter:
    │   delay = min(100ms × 2^attempt, 30s) + random(0, delay × 0.5)
    │
    ├── On reconnect, send re_register:
    │   {"type": "re_register", "app_id": "...", "last_seq": 47,
    │    "pub_key": "ed25519:...", "sig": "ed25519:..."}
    │
    └── Server matches in Postgres, verifies signature, resumes
```

### Jitter for Thundering Herd

When a daemonset pod restarts, all ~110 client pods on that node detect the broken connection simultaneously. Jitter spreads reconnection attempts over a window.

### Daemonset Startup Sequence

1. Start, connect to Postgres
2. Load all `apps` where `status IN ('connected', 'running')` and `server_instance = my_node`
3. Mark them as `status = 'reconnecting'`
4. Wait for clients to reconnect with `re_register`
5. Clients that don't reconnect within configurable window → `status = 'lost_contact'`

`lost_contact` is distinct from `crashed` — the child may still be running fine, just can't reach TRAILS. Once reconnected, everything resumes.

### Client Behavior During Disconnection

`g.status()` and `g.result()` fail silently (return error code, don't block the application). The application continues unimpeded. TRAILS should never cause a business process to block or crash.

---

## 20. Observer Model (Future)

### Concept

Today the daemonset routes messages only between parent and child. In the future, external observers (dashboards, alerting, audit loggers) can subscribe to event streams.

### Three Tiers

| Tier | Scope | Who |
|------|-------|-----|
| Tree participants | Own subtree | Parent/child processes (today) |
| Namespace observers | All events in a namespace | Monitoring apps, dashboards (future) |
| Cluster observers | All events everywhere | SRE tools, compliance recording (future) |

### Implementation Path

The server internally publishes all events to a `tokio::sync::broadcast` channel. Today it has one consumer (parent routing). Adding observer consumers is a natural extension. Observers connect via the `/api/v1/watch` WebSocket with RBAC-gated filtering.

Late-connecting observers catch up from Postgres (historical replay), then switch to the live WebSocket stream (catch-up subscription pattern).

### Design for Today

The internal event bus exists from Phase 1. Observer endpoints can be stubs or minimal implementations. The plumbing is there when needed.

---

## 21. Server Architecture

### Components

```
┌──────────────────────────────────────────────────────────────┐
│  TRAILS Server (Rust binary, tokio async)                     │
│                                                              │
│  ┌─────────────┐  ┌──────────────┐  ┌─────────────────────┐ │
│  │ WebSocket   │  │ REST API     │  │ Auth/RBAC           │ │
│  │ Handler     │  │ (external    │  │ Enforcer            │ │
│  │ :8443/ws    │  │  actors)     │  │                     │ │
│  │             │  │              │  │ - Ed25519 verify    │ │
│  │ client      │  │ :8443/api/v1 │  │ - OAuth/JWT verify  │ │
│  │ connections │  │              │  │ - Role ref resolve  │ │
│  │             │  │              │  │ - Grant check       │ │
│  └──────┬──────┘  └──────┬───────┘  └──────┬──────────────┘ │
│         │                │                  │                │
│         ▼                ▼                  ▼                │
│  ┌──────────────────────────────────────────────────────────┐│
│  │  Internal Event Bus (tokio::sync::broadcast)             ││
│  │  → parent routing (today)                                ││
│  │  → observer fan-out (future)                             ││
│  └──────────────────────┬───────────────────────────────────┘│
│                         ▼                                    │
│  ┌──────────────────────────────────────────────────────────┐│
│  │  Postgres (sqlx)                                         ││
│  │  apps | messages | snapshots | crashes | control_queue   ││
│  │  grants | role_refs | audit_log | active_sessions        ││
│  └──────────────────────────────────────────────────────────┘│
└──────────────────────────────────────────────────────────────┘
```

### Deployment Options

**DaemonSet (one server per node):** Lowest latency (node-local). Connection count naturally bounded by pods-per-node (~110 max). Each instance handles only its local node's pods.

**Deployment with LB (centralized):** Simpler ops. Horizontally scalable. A single Rust async server comfortably holds 50,000–100,000 idle WebSocket connections.

### Connection Capacity

Per idle WebSocket connection: ~5–10 KB (file descriptor + kernel socket buffer + small struct). No threads, no polling — tokio's async model means idle connections cost almost nothing.

| Deployment | Connections/instance | Memory overhead |
|---|---|---|
| DaemonSet, 30 pods/node | ~30 | ~300 KB |
| DaemonSet, 110 pods/node | ~110 | ~1 MB |
| Centralized, 10K pods | ~10,000 | ~100 MB |
| Centralized, 50K pods | ~50,000 | ~500 MB |

---

## 22. Postgres Schema

```sql
-- ═══════════════════════════════════════════════════
-- Core tables
-- ═══════════════════════════════════════════════════

CREATE TABLE apps (
    app_id              UUID PRIMARY KEY,
    parent_id           UUID REFERENCES apps(app_id),
    app_name            TEXT NOT NULL,
    namespace           TEXT,
    pod_name            TEXT,
    node_name           TEXT,
    pod_ip              INET,
    pid                 INTEGER,
    ppid                INTEGER,
    executable          TEXT,

    -- Process identity
    proc_uid            INTEGER,
    proc_gid            INTEGER,
    proc_user           TEXT,

    -- Originator (root actor, inherited from root)
    originator_sub      TEXT,
    originator_groups   TEXT[],

    -- Cryptographic identity
    pub_key             TEXT,             -- child's Ed25519 public key

    -- Lifecycle
    status              TEXT NOT NULL DEFAULT 'scheduled',
        -- scheduled | connected | running | done | error
        -- crashed | cancelled | start_failed
        -- reconnecting | lost_contact
    start_time          TIMESTAMPTZ,
    connected_at        TIMESTAMPTZ,
    disconnected_at     TIMESTAMPTZ,
    server_instance     TEXT,

    -- Configuration
    role_refs           TEXT[],
    metadata_json       JSONB,            -- tags from TRAILS_INFO
    start_deadline      INTEGER DEFAULT 300,

    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_apps_status ON apps(status);
CREATE INDEX idx_apps_parent ON apps(parent_id);
CREATE INDEX idx_apps_namespace ON apps(namespace);
CREATE INDEX idx_apps_name ON apps(app_name);
CREATE INDEX idx_apps_originator ON apps(originator_sub);

-- ═══════════════════════════════════════════════════
-- Messages (data path)
-- ═══════════════════════════════════════════════════

CREATE TABLE messages (
    id                  BIGSERIAL PRIMARY KEY,
    app_id              UUID NOT NULL REFERENCES apps(app_id),
    direction           TEXT NOT NULL,       -- 'in' (app→server) or 'out' (server→app)
    msg_type            TEXT NOT NULL,       -- Status, Result, Error, Control
    seq                 BIGINT NOT NULL,
    correlation_id      TEXT,
    payload_json        JSONB,
    payload_bytes       BYTEA,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_messages_app ON messages(app_id, created_at);
CREATE INDEX idx_messages_type ON messages(app_id, msg_type);

-- ═══════════════════════════════════════════════════
-- Snapshots (state reporting)
-- ═══════════════════════════════════════════════════

CREATE TABLE snapshots (
    id                  BIGSERIAL PRIMARY KEY,
    app_id              UUID NOT NULL REFERENCES apps(app_id),
    namespace           TEXT,
    seq                 BIGINT NOT NULL,
    snapshot_json       JSONB NOT NULL,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_snapshots_latest ON snapshots(app_id, created_at DESC);
CREATE INDEX idx_snapshots_namespace ON snapshots(namespace, created_at DESC);
CREATE INDEX idx_snapshots_gin ON snapshots USING GIN(snapshot_json);

-- ═══════════════════════════════════════════════════
-- Crashes
-- ═══════════════════════════════════════════════════

CREATE TABLE crashes (
    id                  BIGSERIAL PRIMARY KEY,
    app_id              UUID NOT NULL REFERENCES apps(app_id),
    detected_at         TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    crash_type          TEXT NOT NULL,
        -- connection_drop | heartbeat_timeout | never_started
    gap_seconds         REAL,
    metadata_json       JSONB
);

CREATE INDEX idx_crashes_time ON crashes(detected_at DESC);
CREATE INDEX idx_crashes_app ON crashes(app_id);

-- ═══════════════════════════════════════════════════
-- Control queue
-- ═══════════════════════════════════════════════════

CREATE TABLE control_queue (
    id                  BIGSERIAL PRIMARY KEY,
    app_id              UUID NOT NULL REFERENCES apps(app_id),
    action              TEXT NOT NULL,
    payload_json        JSONB,
    sent_at             TIMESTAMPTZ,
    acked_at            TIMESTAMPTZ,
    ack_result_json     JSONB,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_control_pending ON control_queue(app_id) WHERE sent_at IS NULL;

-- ═══════════════════════════════════════════════════
-- RBAC: Role references
-- ═══════════════════════════════════════════════════

CREATE TABLE role_refs (
    name                TEXT PRIMARY KEY,
    description         TEXT,
    grants              JSONB NOT NULL,
        -- [{"sub": "group:sre-team", "roles": ["read", "write", "cancel"]}, ...]
    namespace_scope     TEXT,
    created_by          TEXT NOT NULL,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE role_refs_history (
    id                  BIGSERIAL PRIMARY KEY,
    role_ref_name       TEXT NOT NULL,
    action              TEXT NOT NULL,       -- "created", "updated", "deleted"
    old_grants          JSONB,
    new_grants          JSONB,
    changed_by          TEXT NOT NULL,
    changed_at          TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- ═══════════════════════════════════════════════════
-- RBAC: Per-app grants (inline + resolved from role_refs)
-- ═══════════════════════════════════════════════════

CREATE TABLE grants (
    id                  BIGSERIAL PRIMARY KEY,
    app_id              UUID NOT NULL REFERENCES apps(app_id),
    subject             TEXT NOT NULL,
    roles               TEXT[] NOT NULL,
    granted_by          UUID NOT NULL,       -- app_id of parent that created this
    expires_at          TIMESTAMPTZ,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_grants_app ON grants(app_id);
CREATE INDEX idx_grants_subject ON grants(subject);
CREATE INDEX idx_grants_expiry ON grants(expires_at) WHERE expires_at IS NOT NULL;

-- ═══════════════════════════════════════════════════
-- Audit log
-- ═══════════════════════════════════════════════════

CREATE TABLE audit_log (
    id                  BIGSERIAL PRIMARY KEY,
    timestamp           TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    action              TEXT NOT NULL,
    target_app_id       UUID,
    cascade             BOOLEAN DEFAULT false,
    payload_json        JSONB,
    auth_domain         TEXT NOT NULL,       -- "tree" or "external"
    source_app_id       UUID,
    oauth_subject       TEXT,
    oauth_issuer        TEXT,
    oauth_groups        TEXT[],
    source_ip           INET,
    user_agent          TEXT
);

CREATE INDEX idx_audit_time ON audit_log(timestamp DESC);
CREATE INDEX idx_audit_who ON audit_log(oauth_subject);
CREATE INDEX idx_audit_target ON audit_log(target_app_id);

-- ═══════════════════════════════════════════════════
-- Session management (JWT mitigation)
-- ═══════════════════════════════════════════════════

CREATE TABLE active_sessions (
    id                  BIGSERIAL PRIMARY KEY,
    subject             TEXT NOT NULL,
    token_hash          TEXT NOT NULL,
    issued_at           TIMESTAMPTZ NOT NULL,
    last_used_at        TIMESTAMPTZ NOT NULL,
    expires_at          TIMESTAMPTZ NOT NULL,
    revoked             BOOLEAN DEFAULT false
);

CREATE INDEX idx_sessions_subject ON active_sessions(subject);
CREATE INDEX idx_sessions_expiry ON active_sessions(expires_at) WHERE NOT revoked;
```

---

## 23. REST API

### Query Endpoints

```
GET  /api/v1/apps                              # list active apps
GET  /api/v1/apps?namespace=datascan           # filter by namespace
GET  /api/v1/apps?status=running               # filter by status
GET  /api/v1/apps/{appId}                      # single app state
GET  /api/v1/apps/{appId}/children             # direct children
GET  /api/v1/apps/{appId}/children?recursive=true  # full subtree
GET  /api/v1/apps/{appId}/children?status=start_failed
GET  /api/v1/apps/{appId}/messages             # messages for app
GET  /api/v1/apps/{appId}/messages?type=Result # filter by type
GET  /api/v1/apps/{appId}/snapshots/latest     # latest snapshot
GET  /api/v1/apps/{appId}/snapshots            # snapshot history
GET  /api/v1/crashes                           # recent crashes
GET  /api/v1/crashes?namespace=datascan
GET  /api/v1/crashes?since=2025-02-25T00:00:00Z
```

### Child Registration (Parent → Server)

```
POST /api/v1/children
{
  "parentId":      "aaa-...",
  "appId":         "bbb-...",
  "appName":       "pii-scan-step1",
  "startDeadline": 300,
  "roleRefs":      ["sre-on-call", "monitoring"],
  "grants":        [{"sub": "bob@company.com", "roles": ["read"], "expiresAt": "..."}],
  "tags":          {"dagRun": "daily-2025-02-25"}
}

DELETE /api/v1/children/{appId}     # cancel intent before child starts
```

### Control (Parent/Admin → Child)

```
POST /api/v1/apps/{appId}/control
{
  "action": "cancel",
  "payload": {"reason": "user requested"}
}

POST /api/v1/apps/{appId}/cancel?cascade=true
```

### Admin: Role Refs

```
GET    /api/v1/admin/role-refs
POST   /api/v1/admin/role-refs              # create
PUT    /api/v1/admin/role-refs/{name}       # update
DELETE /api/v1/admin/role-refs/{name}       # delete
GET    /api/v1/admin/role-refs/{name}/history
```

### Watch (Live Event Stream)

```
WS  /api/v1/watch                           # all events
WS  /api/v1/watch?namespace=datascan        # filtered by namespace
WS  /api/v1/watch?app_id={appId}            # single app
WS  /api/v1/watch?parent_id={parentId}      # subtree events
```

---

## 24. Client Libraries (Native per Language)

Each client is written idiomatically in its target language. No FFI. Protocol conformance tests ensure interoperability.

### What Every Client Does

1. Read `TRAILS_INFO` env var, base64-decode JSON (or accept explicit config)
2. Generate ephemeral Ed25519 keypair
3. Open WebSocket connection
4. Send registration message with public key
5. Provide `status()`, `result()`, `error()` methods
6. Provide `on()` handler registration for control commands
7. Provide `on_cancel()` hook
8. Implement exponential backoff reconnection
9. Return no-op client when `TRAILS_INFO` is absent

### Rust Client API

```rust
pub struct TrailsClient { ... }

impl TrailsClient {
    pub fn init() -> Result<Self, TrailsError> { ... }
    pub fn init_with(config: TrailsConfig) -> Result<Self, TrailsError> { ... }
    pub fn status(&self, payload: serde_json::Value) -> Result<(), TrailsError> { ... }
    pub fn result(&self, payload: serde_json::Value) -> Result<(), TrailsError> { ... }
    pub fn error(&self, msg: &str, detail: Option<serde_json::Value>) -> Result<(), TrailsError> { ... }
    pub fn on<F>(&self, action: &str, handler: F)
        where F: Fn(serde_json::Value) -> Result<serde_json::Value, String> + Send + 'static;
    pub fn on_cancel<F>(&self, grace: Duration, hook: F)
        where F: FnOnce(CancelContext) + Send + 'static;
    pub fn create_child(&self, name: &str) -> Result<TrailsConfig, TrailsError> { ... }
    pub fn shutdown(self) -> Result<(), TrailsError> { ... }
}
```

### Python Client API

```python
class TrailsClient:
    @classmethod
    def init(cls) -> 'TrailsClient': ...
    @classmethod
    def init_with(cls, config: TrailsConfig) -> 'TrailsClient': ...
    def status(self, payload: dict): ...
    def result(self, payload: dict): ...
    def error(self, msg: str, detail: dict = None): ...
    def on(self, action: str):         ... # decorator
    def on_cancel(self, grace_seconds: int, hook: Callable): ...
    def create_child(self, name: str) -> TrailsConfig: ...
    def shutdown(self): ...
```

### Go Client API

```go
func Init() (*Client, error)
func InitWith(config TrailsConfig) (*Client, error)
func (c *Client) Status(payload map[string]any) error
func (c *Client) Result(payload map[string]any) error
func (c *Client) Error(msg string, detail map[string]any) error
func (c *Client) On(action string, handler func(map[string]any) (map[string]any, error))
func (c *Client) OnCancel(grace time.Duration, hook func(CancelContext))
func (c *Client) CreateChild(name string) (*TrailsConfig, error)
func (c *Client) Shutdown() error
```

### Java Client API

```java
public class TrailsClient implements AutoCloseable {
    public static TrailsClient init() { ... }
    public static TrailsClient initWith(TrailsConfig config) { ... }
    public void status(String payloadJson) { ... }
    public void result(String payloadJson) { ... }
    public void error(String msg) { ... }
    public void on(String action, Function<JsonObject, JsonObject> handler) { ... }
    public void onCancel(Duration grace, Consumer<CancelContext> hook) { ... }
    public TrailsConfig createChild(String name) { ... }
    public void close() { ... }
}
```

### Native Library Dependencies

| Client | WebSocket | Ed25519 | JSON |
|--------|-----------|---------|------|
| Rust | tokio-tungstenite | ed25519-dalek | serde_json |
| Python | websockets | pynacl / cryptography | json (stdlib) |
| Go | nhooyr/websocket | crypto/ed25519 (stdlib) | encoding/json (stdlib) |
| Java | java-websocket | Bouncy Castle | Jackson/Gson |
| C | libwebsockets | tweetnacl / libsodium | cJSON |

---

## 25. CLI Client (Bash/Terminal)

A single static binary for use in bash scripts and ad-hoc terminal operations.

### Commands

```bash
# ── Identity ──
trails register [--name NAME] [--server EP]
    # Registers this shell as a TRAILS node
    # Outputs env vars to eval

trails child-info [--name NAME]
    # Generates TRAILS_INFO for a child
    # Registers intent with server

# ── Data path ──
trails status  JSON_STRING
trails result  JSON_STRING
trails error   MESSAGE

# ── Control path ──
trails cancel [--cascade] [--uuid ID]
trails send UUID ACTION [--payload JSON]

# ── Query ──
trails children [--status STATUS] [--recursive]
trails tree [--uuid ID]

# ── Wait (distributed waitpid) ──
trails wait --uuid ID [--timeout SECONDS] [--progress]
trails wait --parent ID --any
trails wait --parent ID --all

# ── Auth ──
trails login                    # OAuth flow (future)
trails whoami
```

### Example: Bash Pipeline

```bash
#!/bin/bash
eval $(trails register --name "daily-etl")

CHILD=$(trails child-info --name "gpu-worker")
kubectl run worker --env="TRAILS_INFO=$CHILD" ...

trap 'trails cancel --cascade; exit 1' INT TERM

RESULT=$(trails wait --uuid $CHILD_APP_ID --timeout 3600 --progress)
echo "Result: $RESULT"

trails result '{"steps_completed": 1}'
trails shutdown
```

### Example: Tree Visualization

```
$ trails tree

daily-etl (550e8400) [done] python@airflow-pod-x:12345
├── extract (661f9511) [done ✓] python@airflow-pod-x:12345
├── transform (772a0622) [done ✓] python@airflow-pod-x:12345
├── load (883b1733) [running ▶ 75%] python@airflow-pod-x:12345
│   └── spark-job (994c2844) [running] java@k8s:spark-driver
│       ├── executor-1 (aa5d3955) [running ▶ 80%] java@k8s:spark-exec-1
│       └── executor-2 (bb6e4a66) [running ▶ 70%] java@k8s:spark-exec-2
└── notify (cc7f5b77) [scheduled ⏳] — not started
```

---

## 26. Repository Structure

```
trails/
│
├── README.md
├── LICENSE                          ← Apache 2.0 / MIT dual
├── SPEC.md                          ← this document
├── PROTOCOL.md                      ← wire format, message schemas, signing rules
│
├── server/                          ← Rust server (daemonset)
│   ├── Cargo.toml
│   ├── src/
│   │   ├── main.rs
│   │   ├── ws.rs                    ← WebSocket handler
│   │   ├── api.rs                   ← REST API handlers
│   │   ├── auth.rs                  ← OAuth/RBAC enforcement
│   │   ├── tree.rs                  ← tree resolution, cascade logic
│   │   ├── db.rs                    ← Postgres queries (sqlx)
│   │   ├── watch.rs                 ← /api/v1/watch broadcaster
│   │   └── types.rs                 ← shared types
│   ├── migrations/
│   │   └── 001_init.sql
│   └── Dockerfile
│
├── client-rust/                     ← native Rust client
│   ├── Cargo.toml                   ← crates.io: trails-client
│   └── src/
│
├── client-python/                   ← native Python client
│   ├── pyproject.toml               ← PyPI: trails
│   ├── trails/
│   │   ├── __init__.py
│   │   └── client.py
│   └── tests/
│
├── client-go/                       ← native Go client (no cgo)
│   ├── go.mod                       ← github.com/gssmahadevan/trails/client-go
│   ├── trails.go
│   └── trails_test.go
│
├── client-java/                     ← native Java client
│   ├── pom.xml                      ← Maven Central: io.trails:trails-client
│   └── src/
│
├── client-c/                        ← native C client
│   ├── CMakeLists.txt
│   ├── src/
│   └── include/trails.h
│
├── client-cli/                      ← bash-facing CLI tool
│   ├── Cargo.toml                   ← single static binary
│   └── src/
│
├── client-kotlin/                   ← future: Android
├── client-swift/                    ← future: iOS
│
├── conformance/                     ← shared protocol test suite
│   ├── README.md
│   ├── tests/
│   │   ├── 001_register.json
│   │   ├── 002_status.json
│   │   ├── 003_result.json
│   │   ├── 004_cancel_cascade.json
│   │   ├── 005_reconfig_ack.json
│   │   ├── 006_unknown_command.json
│   │   ├── 007_reconnect.json
│   │   ├── 008_signature_verify.json
│   │   └── 009_multi_identity.json
│   └── runner/                      ← test server for conformance
│       └── src/
│
├── deploy/                          ← K8s manifests
│   ├── helm/
│   │   └── trails/
│   │       ├── Chart.yaml
│   │       ├── values.yaml
│   │       └── templates/
│   └── manifests/
│       ├── daemonset.yaml
│       ├── deployment.yaml
│       ├── service.yaml
│       ├── postgres.yaml
│       └── rbac.yaml
│
└── examples/
    ├── python-etl/
    ├── go-microservice/
    ├── java-batch/
    ├── rust-worker/
    ├── airflow-dag/
    └── bash-pipeline/
```

### Versioning

Protocol version (`"v": 1` in `TRAILS_INFO`) is shared across all clients. Client library versions are independent but declare which protocol version they support.

### CI/CD

Each client has its own CI pipeline. The conformance test suite (`conformance/`) runs against every client to prevent drift. If the Go client and Java client both pass all conformance tests, they're interoperable by definition.

---

## 27. Overhead Analysis

TRAILS is designed for zero marginal cost in the common case.

### Per-Pod Overhead

| Resource | Cost (typical job) |
|---|---|
| Network | 3–5 messages total (register, 1–2 status, 1 result). No heartbeats. |
| Persistent connections | 1 idle WebSocket (~5–10 KB). 0 if using HTTP-only data path. |
| CPU | ~0 (no background threads if no heartbeat, no polling) |
| Memory | ~50–100 KB (WebSocket buffer, config, keypair) |
| Disk (Postgres) | 1 row per meaningful business event |
| Server-side state | 1 connection handle per pod |

### Comparison: TRAILS vs One Log Line

A single application log line (200 bytes, sent to Fluentd, stored in Elasticsearch) costs more network, CPU, and disk than TRAILS's entire per-job communication. If an application is already logging, TRAILS is lost in the noise.

### No-Op Client

When `TRAILS_INFO` is not set, `trails_init()` returns a no-op client where all methods silently succeed. Zero overhead — no connection, no threads, no memory. Developers leave TRAILS calls in unconditionally.

### Adaptive Keepalive (Non-K8s Only)

When TRAILS is the only liveness mechanism (bare metal, VMs, mobile), an optional keepalive can be enabled. Default: every 5 minutes. Configurable. Not required when K8s or OS-level health monitoring exists.

---

## 28. Comparison with Existing Systems

### TRAILS vs Unix Process Model

| Aspect | Unix | TRAILS |
|---|---|---|
| Identity | PID (local, 16-bit, recycled) | UUID v4 (global, permanent) |
| Tree | ppid (ephemeral, kernel memory) | Postgres (persistent, queryable) |
| Cross-machine | No | Yes |
| Kill tree | `kill -pgid` (one machine) | Cascade cancel (any boundary) |
| Result delivery | `waitpid()` returns int | Structured JSON result |
| Control | 31 signals, no payload | Open vocabulary, JSON payload, ack response |
| History | Gone when process exits | Permanent in Postgres |

### TRAILS vs K8s

| Aspect | K8s | TRAILS |
|---|---|---|
| Lifecycle | Pod phase (5 states) | Custom state machine with business context |
| Results | Exit code 0/1 | Structured JSON |
| Errors | Exit code 1 + logs | Structured error with checkpoint |
| Control | Delete pod | Open command vocabulary |
| Tree | Owner refs (1 level) | Arbitrary depth, cross-boundary |
| Cancel tree | Delete namespace (one scope) | Cascade (any scope) |
| Cross-platform | K8s only | Any networked process |

### TRAILS vs D-Bus / XPC / COM / Binder

| Aspect | D-Bus | TRAILS |
|---|---|---|
| Scope | Single machine | Network-wide |
| Persistence | None | Postgres |
| Tree awareness | None | Parent-child tree |
| Open vocabulary | Yes (interfaces) | Yes (actions) |
| Cross-platform | Linux only | Any OS |
| Introspection | Yes (Introspectable) | Future (capabilities field) |

### TRAILS vs Airflow XCom

| Aspect | XCom | TRAILS |
|---|---|---|
| Coupling | Airflow only | Any orchestrator |
| Direction | Unidirectional (push/pull) | Bidirectional |
| Timing | After task completion | Anytime during execution |
| Data size | Small metadata (KB) | JSON + binary |
| Audience | Next task in DAG | Any authorized observer |
| Control path | None | Open vocabulary commands |
| Tree depth | Flat (task → task) | Arbitrary depth |
| Cross-orchestrator | No | Yes |

### TRAILS vs gRPC / NATS / ZeroMQ

| Property | gRPC | NATS | TRAILS |
|---|---|---|---|
| Network span | ✓ | ✓ | ✓ |
| Tree-aware | ✗ | ✗ | ✓ |
| Persistent | ✗ | Optional | ✓ (Postgres) |
| Cross-platform | ✓ | ✓ | ✓ |
| Open vocabulary | ✓ (protobuf) | ✓ (subjects) | ✓ (actions) |
| Tree-scoped auth | ✗ | ✗ | ✓ |

### The Unique Combination

No existing system provides: **network-spanning + tree-aware + persistent + cross-platform + open vocabulary + tree-scoped authorization**. Each existing system has at most three of these six properties.

---

## 29. Phase Plan

| Phase | Scope | Deliverable |
|-------|-------|------------|
| **Phase 1** | Server (WebSocket + lifecycle + Postgres schema) + Rust client + Python client + conformance tests | Working lifecycle awareness + data path |
| **Phase 2** | REST API + Go client + CLI client (`trails` binary) + `trails tree` + `trails wait` | Orchestrator integration + bash support |
| **Phase 3** | Bidirectional control path + open command vocabulary + `on_cancel` hooks + cascade cancel | Full control path + tree-wide operations |
| **Phase 4** | Java client + C client + Helm chart | Full language coverage + easy deploy |
| **Phase 5** | OAuth/OIDC integration + role refs + RBAC enforcement + audit log | Enterprise security |
| **Phase 6** | Observer model + `/api/v1/watch` fan-out + Grafana dashboard | Observability integration |
| **Future** | Kotlin client (Android) + Swift client (iOS) + federation (multi-cluster) | Mobile + scale |

---

## Name

**TRAILS** — **T**ree-scoped **R**elay for **A**pplication **I**nfo, **L**ifecycle, and **S**ignaling

### Meaning

The name encodes what the system does:

- **The process tree trail** — parent to child to grandchild, the lineage path that TRAILS makes visible and persistent across any infrastructure boundary.
- **The audit trail** — every action, result, cancel, and command, permanently in Postgres with originator identity.
- **The breadcrumb trail** — snapshots and checkpoints a child leaves behind so the parent can follow what happened, resume from where it stopped, or understand why it failed.
- **The control trail** — cascade cancel follows the trail from root to leaves, bottom-up, across any boundary.

### Tribute

The name is composed from the initials of the gurus whose work made all of this possible:

- **T** — Andrew **T**anenbaum — created Minix, wrote the foundational OS textbooks, inspired a generation
- **R** — **R**ichard Stevens — wrote *Unix Network Programming* and *Advanced Programming in the UNIX Environment*, the definitive references that taught the world how Unix systems work
- **A** — **A**ndrew Tanenbaum — his first name completes the tribute to the teacher whose Minix sparked Linux
- **I** — **I**ngo Molnár — created the Completely Fair Scheduler (CFS), the real-time preemption framework, and fundamental Linux kernel infrastructure
- **L** — **L**inus Torvalds — created Linux, the kernel on which most of the world's infrastructure runs
- **S** — Richard **S**tallman / Richard **S**tevens — Stallman created the free software movement, GCC, and the GNU tools without which Linux would have no userspace; Stevens wrote the books that made Unix systems programming accessible to all

गुरुर्ब्रह्मा गुरुर्विष्णुः गुरुर्देवो महेश्वरः ।
गुरुः साक्षात् परब्रह्म तस्मै श्री गुरवे नमः ॥

*The Guru is Brahma (creator), the Guru is Vishnu (preserver), the Guru is Shiva (transformer). The Guru is the supreme reality itself. To that Guru, I offer my salutations.*

These gurus created, preserved, and transformed the systems we build upon. Every invocation of `trails` is a silent namah.

Lowercase `trails` used as CLI command and package name.

---

*This spec captures the complete design as evolved through iterative discussion. PROTOCOL.md (to be written separately) will contain the precise wire format, message schemas, signing rules, and canonical serialization required for implementing conforming clients.*