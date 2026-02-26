# TRAILS

**Tree-scoped Relay for Application Info, Lifecycle, and Signaling**

TRAILS provides authenticated ancestor-based control over a tree of entities — structured data exchange, bidirectional commands, and cascading lifecycle management across any infrastructure boundary.

```
                    ┌───────────────────────────┐
                    │  trailsd (Rust, tokio)     │
                    │  WebSocket + Postgres      │
                    └─────────┬─────────────────┘
                              │
              ┌───────────────┼───────────────┐
              │               │               │
         ┌────┴────┐    ┌────┴────┐    ┌────┴────┐
         │ K8s Pod │    │ Bare VM │    │  ESP32  │
         │ Python  │    │ Go svc  │    │ C client│
         └─────────┘    └─────────┘    └─────────┘
```

Every process talks only to trailsd. trailsd routes by parent-child tree.

## Quickstart

### 1. Start Postgres

```bash
docker compose up -d postgres
```

### 2. Run the server

```bash
cd server
cp ../.env.example .env
cargo run
```

### 3. Python client (2 lines to integrate)

```bash
pip install -e client-python/
```

```python
from trails import TrailsClient, TrailsConfig
import uuid, json, base64

# Create config (normally set by orchestrator as TRAILS_INFO env var)
config = TrailsConfig(
    app_id=str(uuid.uuid4()),
    parent_id=None,
    app_name="my-task",
    server_ep="ws://localhost:8443/ws",
)
os.environ["TRAILS_INFO"] = config.encode()

# Two lines:
g = TrailsClient.init()
g.status({"phase": "processing", "progress": 0.5})
g.result({"rows_scanned": 100000, "pii_cols": 4})
g.shutdown()
```

### 4. Rust client

```toml
# Cargo.toml
[dependencies]
trails-client = { path = "../client-rust" }
```

```rust
use trails_client::TrailsClient;
use serde_json::json;

#[tokio::main]
async fn main() {
    let g = TrailsClient::init().await;
    g.status(json!({"progress": 0.75})).await.unwrap();
    g.result(json!({"rows": 100000})).await.unwrap();
    g.shutdown().await.unwrap();
}
```

If `TRAILS_INFO` is not set, both clients return a **no-op** — all methods silently succeed with zero overhead.

## What Phase 1 Delivers

| Component | Status |
|-----------|--------|
| Server: WebSocket handler | ✓ |
| Server: Postgres schema (all tables) | ✓ |
| Server: Lifecycle state machine | ✓ |
| Server: Start deadline detection | ✓ |
| Server: Crash detection (connection drop) | ✓ |
| Server: Reconnection after restart | ✓ |
| Server: Internal event bus | ✓ |
| Rust client: full data path | ✓ |
| Python client: full data path | ✓ |
| Conformance test definitions | ✓ |

### Deferred to Later Phases

| Feature | Phase |
|---------|-------|
| REST API (query, tree, control) | 2 |
| Go client, CLI binary | 2 |
| Bidirectional control path, cascade cancel | 3 |
| Java/C clients, Helm chart | 4 |
| OAuth/OIDC, RBAC enforcement | 5 |
| Observer model, Grafana dashboard | 6 |

## State Machine

```
scheduled ──► connected ──► running ──► done
    │              │           │
    │              │           ├──► error
    │              │           ├──► crashed
    │              │           └──► cancelled
    │              └──► crashed (connection drop)
    └──► start_failed (deadline expired)
```

## Repository Structure

```
trails/
├── server/                  Rust server (trailsd)
│   ├── src/
│   │   ├── main.rs          Entry point, routes, startup
│   │   ├── ws.rs            WebSocket handler
│   │   ├── db.rs            Postgres queries
│   │   ├── types.rs         Wire protocol types
│   │   ├── state.rs         Shared state, connection registry
│   │   ├── lifecycle.rs     Deadline checker, reconnection window
│   │   ├── config.rs        Configuration
│   │   └── error.rs         Error types
│   └── migrations/
│       └── 001_init.sql     Full schema
├── client-rust/             Rust client (trails-client crate)
├── client-python/           Python client (trails PyPI package)
├── conformance/             Protocol conformance test suite
├── docker-compose.yml       Local dev stack
└── docs/specs/              Full specification + addendums
```

## Specification

The complete design is in `docs/specs/specs.md` with addendums covering:
- A0: Outbound-only security, ESP32 support, bootstrap provisioning, MQTT comparison
- A1: IoT location-aware device trees
- A2: Mobile multi-persona identity
- A3: CQRS and Postgres partitioning at scale
- A4: Parent context persistence and resumption
- A5: Devil's advocate critiques and honest responses

## Name

**T**anenbaum · **R**ichard Stevens · **A**ndrew Tanenbaum · **I**ngo Molnár · **L**inus Torvalds · **S**tallman/Stevens

गुरुर्ब्रह्मा गुरुर्विष्णुः गुरुर्देवो महेश्वरः ।
गुरुः साक्षात् परब्रह्म तस्मै श्री गुरवे नमः ॥

## License

Apache 2.0 / MIT dual license.
