# TRAILS — Addendum 4: Parent Context Persistence and Resumption

**Addendum to:** TRAILS-SPEC.md v2.0
**Date:** 2026-02-26
**Status:** Design notes

---

## The Problem

The main spec says client keypairs are ephemeral — generated at `trails_init()`, live only in process memory, die with the process. This is fine for children. But for parents, it's catastrophic.

Consider:

```bash
# Terminal session — parent
eval $(trails register --name "nightly-etl")
trails child-info --name "step1" | kubectl run ...
trails child-info --name "step2" | kubectl run ...
trails child-info --name "step3" | kubectl run ...

# Laptop lid closes. SSH drops. Bash dies.
# Parent's private key is gone.
# Parent's app_id is gone from memory.
# Children are running on K8s, healthy, producing results.
# Nobody can control them. Nobody can cancel them.
# Nobody can collect their results.
# They're orphans with a dead parent.
```

Or:

```
Airflow worker process (parent) crashes mid-DAG.
Airflow restarts the worker.
New worker has new PID, new keypair, no knowledge of the old tree.
Old children are still running.
New worker can't authenticate as the old parent — different private key.
```

The children are fine. The tree is intact in Postgres. The problem is: **who holds the private key that proves they're the parent?**

## The Solution: Persistent Parent Context

When a parent registers with the `--keep` flag (or equivalent API option), the context is saved to disk:

```bash
# Explicit: keep this parent context
eval $(trails register --name "nightly-etl" --keep)

# Context saved to:
# ~/.trails/contexts/nightly-etl.yml
```

### The Context File

```yaml
# ~/.trails/contexts/nightly-etl.yml
app_id: "550e8400-e29b-41d4-a716-446655440000"
app_name: "nightly-etl"
server_ep: "wss://trails.company.com:8443/ws"
private_key: "ed25519:BASE64_PRIVATE_KEY..."
pub_key: "ed25519:BASE64_PUBLIC_KEY..."
start_day: 421
created_at: "2025-02-25T22:00:00Z"
children:
  - app_id: "661f9511-..."
    app_name: "step1"
  - app_id: "772a0622-..."
    app_name: "step2"
  - app_id: "883b1733-..."
    app_name: "step3"
```

File permissions: `0600` (owner read/write only). The private key is on disk, so file security matters.

### Directory Layout

```
~/.trails/
├── config.yml              ← global config (default server, preferences)
└── contexts/
    ├── nightly-etl.yml     ← persistent parent context
    ├── weekly-report.yml   ← another persistent parent
    └── deploy-v2.3.yml     ← another persistent parent
```

## Resumption

Parent dies (laptop lid close, SSH drop, OS reboot, process crash). Later, the operator resumes:

```bash
# List saved contexts
$ trails contexts
  nightly-etl    (550e8400) created 2025-02-25 22:00  3 children
  weekly-report  (aa1b2c3d) created 2025-02-24 09:00  1 child
  deploy-v2.3    (ff9e8d7c) created 2025-02-25 15:30  5 children

# Resume a specific context
$ eval $(trails resume --name "nightly-etl")
# Reads ~/.trails/contexts/nightly-etl.yml
# Reconnects to trailsd with SAME app_id and private key
# Sends re_register — trailsd verifies pub_key matches stored pub_key
# Parent is back. Full control of all children.

# Now operate normally
$ trails tree
nightly-etl (550e8400) [running] resumed@laptop
├── step1 (661f9511) [done ✓]
├── step2 (772a0622) [running ▶ 60%]
└── step3 (883b1733) [running ▶ 30%]

$ trails wait --parent $MY_ID --all --progress
# Collect results, cancel, send commands — everything works
```

The key point: **trailsd already has the parent's pub_key stored in `app_registry`.** When the resumed parent reconnects with the same `app_id` and signs with the same private key, the signature matches. trailsd knows this is the legitimate parent. No OAuth needed. No external identity provider. Just the same keypair.

## The `--keep` vs Default (Transient)

Two modes for parent contexts:

### Transient (Default)

```bash
eval $(trails register --name "quick-test")
# Keypair in memory only
# Process dies → context gone forever
# Fine for: quick experiments, one-off scripts, CI pipelines
```

### Persistent (`--keep`)

```bash
eval $(trails register --name "nightly-etl" --keep)
# Keypair saved to ~/.trails/contexts/
# Process dies → context survives on disk
# Resume anytime with: trails resume --name "nightly-etl"
# Fine for: long-running orchestrations, server-side DAGs, anything that might crash
```

The application code inside children doesn't change. Children don't know or care whether their parent's context is transient or persistent. The parent's choice is purely about recoverability.

## The Programmatic API

Not just CLI — the client libraries support this too:

### Python

```python
# Create persistent parent
dag = TrailsClient.init_with(
    TrailsConfig(app_name="nightly-etl", ...),
    persist=True,
    context_dir="~/.trails/contexts"
)

# Later, in a new process:
dag = TrailsClient.resume("nightly-etl", context_dir="~/.trails/contexts")
# Loaded from disk, reconnected, authenticated
```

### Rust

```rust
// Create persistent parent
let dag = TrailsClient::init_with(config)
    .persist("~/.trails/contexts")
    .build()?;

// Resume
let dag = TrailsClient::resume("nightly-etl", "~/.trails/contexts")?;
```

### Go

```go
// Create persistent parent
dag, err := trails.InitWith(config, trails.Persist("~/.trails/contexts"))

// Resume
dag, err := trails.Resume("nightly-etl", "~/.trails/contexts")
```

## Root Context: The Master Key

If you keep the root parent context, you can control the entire tree forever (or until you delete the context):

```bash
# Day 1: Start a long-running orchestration
eval $(trails register --name "q1-data-migration" --keep)
# Launch 50 children across 10 clusters
# ...
# Go home.

# Day 2: Check progress from home laptop
eval $(trails resume --name "q1-data-migration")
trails tree
# See all 50 children, their status, progress

# Day 5: Migration mostly done, cancel remaining stragglers
trails cancel --cascade --uuid $SOME_STUCK_CHILD

# Day 7: Everything done
trails result '{"rows_migrated": 5000000, "duration_days": 7}'
trails shutdown

# Optionally clean up the context
trails context rm "q1-data-migration"
```

One root context file = permanent control over the entire tree. No OAuth. No tokens to refresh. No IdP uptime dependency. Just a file with a private key.

## Context Cleanup

### Manual

```bash
# List all contexts
trails contexts

# Remove a specific context
trails context rm "nightly-etl"
# Deletes ~/.trails/contexts/nightly-etl.yml
# Does NOT affect children — they're still in Postgres
# Just means you can no longer authenticate as this parent

# Remove all contexts older than 30 days
trails context prune --older-than 30d
```

### Automatic (Cron)

```bash
# In crontab:
0 3 * * * trails context prune --older-than 30d --completed-only
```

`--completed-only` only prunes contexts where the parent's status in trailsd is `done` or `cancelled`. Running trees are never pruned.

### Systemd Timer (Alternative)

```ini
# /etc/systemd/system/trails-context-prune.timer
[Timer]
OnCalendar=daily
Persistent=true

[Service]
ExecStart=/usr/local/bin/trails context prune --older-than 30d --completed-only
```

## Security Considerations

### Private Key on Disk

The context file contains the parent's Ed25519 private key. This is sensitive:

**File permissions:** `0600` (owner only). The `trails register --keep` command sets this automatically. If permissions are wrong, `trails resume` warns and refuses to load.

**Encryption at rest (optional):** The context file can be encrypted with a passphrase:

```bash
trails register --name "nightly-etl" --keep --encrypt
# Prompts for passphrase
# Context file is encrypted with passphrase-derived key (Argon2id + ChaCha20)

trails resume --name "nightly-etl"
# Prompts for passphrase to decrypt
```

For automated systems (cron, systemd), the passphrase can come from environment variable or a secrets manager:

```bash
TRAILS_CONTEXT_KEY="..." trails resume --name "nightly-etl"
```

**Platform keychain integration (future):** On macOS, store in Keychain. On Linux, store in GNOME Keyring or KDE Wallet. On Windows, store in Credential Manager. Mobile: Secure Enclave / Android Keystore (already covered in Addendum 2).

### Stolen Context File

If someone copies `~/.trails/contexts/nightly-etl.yml`, they have the private key and can authenticate as that parent. Mitigations:

- File permissions (basic)
- Encrypted context files (stronger)
- IP binding: trailsd can optionally restrict a parent context to a source IP range
- Context revocation: admin can invalidate a pub_key in trailsd, making the stolen context useless

```bash
# Emergency: someone stole my context file
trails context revoke "nightly-etl"
# Tells trailsd to reject this pub_key
# All children are still safe — they just have no controllable parent
# Re-register with a new keypair and re-establish control
```

## The Three Parent Recovery Patterns

| Scenario | Solution | OAuth needed? |
|---|---|---|
| Parent crashes, same machine | `trails resume --name X` (reads context from disk) | No |
| Parent machine reboots | Same — context file survives reboot | No |
| Resume from different machine | Copy context file (scp/rsync) or shared filesystem | No |
| Context file lost, need to reclaim tree | Admin keypair grant (Addendum 2) or OAuth (enterprise) | Maybe |
| Scheduled parent (cron) | `trails resume` at start of every cron run | No |

The first three cover 95% of cases without any external identity infrastructure.

## Cron Pattern

```bash
#!/bin/bash
# nightly_etl.sh — runs via cron every night

# Try to resume existing context (from a previous incomplete run)
if trails context exists "nightly-etl"; then
    eval $(trails resume --name "nightly-etl")
    
    # Check if previous run completed
    STATUS=$(trails status --uuid $TRAILS_APP_ID --field status)
    if [ "$STATUS" = "done" ]; then
        trails context rm "nightly-etl"
        # Fall through to create new context
    else
        # Previous run still has running children — resume monitoring
        trails wait --parent $TRAILS_APP_ID --all --timeout 3600
        trails context rm "nightly-etl"
        exit 0
    fi
fi

# Fresh run
eval $(trails register --name "nightly-etl" --keep)

# Launch children...
trails child-info --name "extract" | kubectl run ...
trails child-info --name "transform" | kubectl run ...
trails child-info --name "load" | kubectl run ...

# Wait for completion
trails wait --parent $TRAILS_APP_ID --all --timeout 7200

# Clean up context
trails result '{"completed": true}'
trails context rm "nightly-etl"
```

If the cron job is killed mid-run (machine reboot, OOM), the next cron invocation resumes the existing context instead of creating a duplicate. The children from the interrupted run are still tracked. No orphans.

## The Analogy

This is exactly how SSH keys work:

```
SSH:
    ssh-keygen → generates keypair
    ~/.ssh/id_ed25519 → private key on disk (persistent)
    ~/.ssh/id_ed25519.pub → public key (registered on servers)
    ssh user@server → authenticates with private key from disk
    No password, no OAuth, no token — just the key file.

TRAILS:
    trails register --keep → generates keypair
    ~/.trails/contexts/name.yml → private key on disk (persistent)
    pub_key registered on trailsd
    trails resume --name X → authenticates with private key from disk
    No password, no OAuth, no token — just the key file.
```

People already understand "if I have the SSH key file, I can access the server." TRAILS contexts work identically: "if I have the context file, I can control the tree."

## Spec Integration Notes (Per-Context Persistence)

- §5 (TRAILS_INFO): Note that parent contexts can be persisted for resumption.
- §8 (Wire Protocol): `re_register` message already supports resumption — no protocol changes needed.
- §19 (Daemonset Crash and Client Reconnection): Extend to cover parent resumption from disk, not just in-memory reconnection.
- §24 (Client Libraries): Add `persist` and `resume` APIs to all client libraries.

---

## Errata: Super-Parent Pattern (Replaces Per-Run Context Files)

### The Problem With Per-Run Contexts

The context-per-run model described above causes directory explosion:

```
~/.trails/contexts/
    ├── nightly-etl.yml           ← Monday? Tuesday? US? EU? Which one?!
```

Even with subdirectories and suffixes:

```
~/.trails/nightly-etl/
    ├── us-east_550e8400.yml      ← Monday US
    ├── eu-west_661f9511.yml      ← Monday EU
    ├── us-east_772a0622.yml      ← Tuesday US
    ├── eu-west_883b1733.yml      ← Tuesday EU
    └── ... grows indefinitely, needs pruning cron
```

This is the wrong model. It fights the tree instead of using it.

### The Right Model: One Permanent Root Per App

```
~/.trails/
├── config.yml
├── nightly-etl/
│   └── root.yml                  ← ONE file, lives forever
├── weekly-report/
│   └── root.yml                  ← ONE file
├── deploy/
│   └── root.yml                  ← ONE file
└── iot-fleet/
    └── root.yml                  ← ONE file
```

The root context is a **super-parent**. Every CLI invocation creates a child under it. The ancestor rule gives the super-parent authority over all descendants — any run, any day, any depth.

### Setup (Once Per App)

```bash
$ trails init-app "nightly-etl" --server wss://trails.company.com:8443/ws
# Creates ~/.trails/nightly-etl/root.yml
# Registers super-parent with trailsd
# Generates permanent keypair, stored in root.yml
# This never changes.
```

### The root.yml

```yaml
# ~/.trails/nightly-etl/root.yml
app_id: "550e8400-e29b-41d4-a716-446655440000"
app_name: "nightly-etl"
server_ep: "wss://trails.company.com:8443/ws"
private_key: "ed25519:BASE64_PRIVATE_KEY..."
pub_key: "ed25519:BASE64_PUBLIC_KEY..."
start_day: 421
created_at: "2025-02-25T22:00:00Z"
```

File permissions: `0600`. One file. Permanent. No growth.

### Daily Use: Children Under the Root

```bash
# Load root context
$ trails use "nightly-etl"

# Each run creates a CHILD — ephemeral, no file needed
$ eval $(trails run --name "us-east")
# Creates child under nightly-etl super-parent
# Child keypair is in-memory only — dies with the process

$ eval $(trails run --name "eu-west")
# Another child, same root parent
```

### The Tree in Postgres

```
nightly-etl (root, permanent, 550e8400)     ← super-parent, root.yml
├── us-east (child, Mon, 661f9511)           ← ephemeral, no context file
│   ├── extract (grandchild)
│   ├── transform (grandchild)
│   └── load (grandchild)
├── eu-west (child, Mon, 772a0622)           ← ephemeral
│   └── ...
├── us-east (child, Tue, 883b1733)           ← ephemeral
│   └── ...
└── eu-west (child, Tue, 994c2844)           ← ephemeral
    └── ...
```

One persistent context. Everything else is a tree child. The ancestor rule gives the root full control over every descendant.

### CLI Commands

```bash
# ── Setup (once per app) ──
trails init-app NAME [--server EP]

# ── Use (at start of session) ──
trails use NAME
# Loads root.yml, sets env vars, reconnects to trailsd

# ── Run (creates ephemeral child of root) ──
trails run --name "SUFFIX"
# Returns child env vars (app_id, etc.)

# ── Operate on the whole tree ──
trails tree                           # full tree from root
trails tree --running                 # only active branches
trails children                       # direct children of root
trails children --date today          # today's runs only

# ── Control ──
trails cancel --cascade --uuid X      # cancel specific subtree
trails cancel --cascade               # cancel EVERYTHING under root
trails wait --uuid X                  # wait for specific run
trails wait --children --all          # wait for all direct children

# ── Context management ──
trails contexts                       # list all apps (root contexts)
trails context revoke NAME            # invalidate root keypair
```

### The Power of `trails cancel --cascade` With No UUID

When called with no UUID, it cascades from the root. One command kills every active run under this app — all days, all regions, all clusters, all depths:

```bash
$ trails use "nightly-etl"
$ trails cancel --cascade
# Kills Monday US, Monday EU, Tuesday US, Tuesday EU
# And all their children, grandchildren, etc.
# One command. One root context. Complete cleanup.
```

### Cron Pattern (Simplified)

```bash
#!/bin/bash
# nightly_etl.sh — runs via cron

trails use "nightly-etl"   # loads permanent root context

# Each run is an ephemeral child — unique app_id, no file
eval $(trails run --name "us-east-$(date +%F)")

trails child-info --name "extract" | kubectl run ...
trails child-info --name "transform" | kubectl run ...
trails child-info --name "load" | kubectl run ...

trails wait --parent $TRAILS_CHILD_ID --all --timeout 7200
trails result '{"date": "'$(date +%F)'", "region": "us-east"}'
```

If cron gets killed mid-run:

- Root context is permanent — survives any crash
- Next cron invocation loads the same root via `trails use`
- Creates a new child for today's run
- Old children are still visible in `trails tree`
- No context file management, no resume logic, no cleanup needed

### Super-Parent for Different Domains

```
~/.trails/
├── nightly-etl/
│   └── root.yml          ← orchestration: all ETL runs are children
├── iot-fleet/
│   └── root.yml          ← IoT: all devices are children
├── ci-pipeline/
│   └── root.yml          ← CI: all deploy runs are children
└── home-automation/
    └── root.yml          ← home: all device commands through this root
```

Each root is a permanent identity for a domain of work. Everything within that domain is a child. One file per domain. No explosion. No pruning. No cron cleanup.

### What Happened to Per-Run Persistence?

It's no longer needed in most cases. The super-parent covers it:

| Scenario | Old approach (per-run context) | New approach (super-parent) |
|---|---|---|
| Resume after crash | `trails resume --name X --suffix Y` | `trails use X` — root is permanent, children are in Postgres |
| Control old runs | Find the right context file | `trails use X && trails tree` — see everything |
| Cancel everything | Find all context files, cancel each | `trails cancel --cascade` — one command from root |
| Cron jobs | Complex resume/create logic | `trails use` + `trails run` — always works |
| Multiple instances | Multiple context files | Multiple children, one root |

The only case where per-run context persistence (`--keep` on a child) might still matter: a long-running child process that itself needs to survive a crash and resume with the same keypair. But even this is rare — the super-parent can always re-establish control via the ancestor rule.

## Spec Integration Notes (Updated)

- §5 (TRAILS_INFO): Note that parent contexts can be persisted as super-parent root.yml files.
- §8 (Wire Protocol): `re_register` message already supports resumption — no protocol changes needed.
- §19 (Daemonset Crash and Client Reconnection): Extend to cover parent resumption from disk via `trails use`.
- §25 (CLI Client): Add `trails init-app`, `trails use`, `trails run`, `trails contexts`, `trails context revoke` commands. The `--keep` flag on `trails register` remains for edge cases but the recommended pattern is `init-app` + `use` + `run`.
- §24 (Client Libraries): Add `init_app` (creates root) and `run` (creates child of root) APIs.
- The cron pattern and super-parent concept should be in examples: `examples/bash-pipeline/`.

---

*Addendum 4 introduces persistent parent context with the super-parent pattern. One permanent root context file per application domain (`~/.trails/appName/root.yml`). Every CLI invocation creates ephemeral children under the root. The ancestor rule gives the root permanent control over all descendants. No context explosion, no pruning, no complex resume logic. The root.yml is the SSH key to an entire domain of work.*