# TRAILS Conformance Test Suite

Protocol conformance tests ensure interoperability between all TRAILS client
implementations. If the Rust client and Python client both pass all conformance
tests, they are interoperable by definition.

## Structure

Each `.json` file defines a test scenario with:

- **name** — human-readable test name
- **description** — what the test validates
- **steps** — ordered sequence of actions
  - `client_send` — message the client sends to the server
  - `server_expect` — what the server should have received
  - `server_send` — message the server sends back
  - `client_expect` — what the client should have received
  - `db_check` — SQL condition to verify in Postgres
  - `delay` — wait N seconds (for deadline tests)

## Running

```bash
# Against a running trailsd + Postgres:
python conformance/runner.py --server ws://localhost:8443/ws

# Or with the test harness (starts server automatically):
cargo run -p conformance-runner -- --all
```

## Adding Tests

1. Create `NNN_testname.json`
2. Follow the step schema
3. Run against all client implementations

## Phase 1 Tests

| Test | Validates |
|------|-----------|
| 001_register | Fresh registration → connected state |
| 002_status | Status message → running state + snapshot stored |
| 003_result | Result message → done state + message stored |
| 004_error | Error message → error state |
| 005_disconnect | Graceful disconnect → done state, no crash |
| 006_crash_detection | Connection drop → crashed state + crash record |
| 007_reconnect | Re-registration after server restart |
