# TRAILS — Addendum 0

**Addendum to:** TRAILS-SPEC.md v2.0
**Date:** 2026-02-26
**Status:** Design notes collected during spec review

---

## A0.1 — Outbound-Only Security Model

The TRAILS client **never opens a listening socket.** It never calls `bind()`, never calls `listen()`, never calls `accept()`. The attack surface from the network side is zero.

The child initiates a single outbound WebSocket connection to the TRAILS server (trailsd). That connection is long-lived and idle by default — zero traffic when no business messages to send.

### Why This Matters

| Approach | Listening socket? | Attack surface |
|---|---|---|
| HTTP callback server in child | Yes | Port scan, unauthorized requests, DoS |
| gRPC service in child | Yes | Same |
| Redis/NATS sidecar | No, but requires exposed infra | Cluster infrastructure exposed |
| K8s exec/port-forward | Yes (kubelet) | Requires API access, privilege escalation |
| **TRAILS client** | **No — outbound only** | **Zero from network side** |

### Implications

- Child pods can run with the strictest possible K8s NetworkPolicy: deny all ingress, allow outbound to `trails-system:8443` only.
- Passes security review instantly in regulated environments (banking, healthcare). No listening port = no inbound attack vector assessment needed.
- Firewall rules are trivially simple at every level — OS, K8s, cloud security group.
- No port conflicts. No port allocation. No `containerPort` declarations for TRAILS.

Recommended for inclusion in §3 (Core Design Principles) as principle 11:

> **11. Outbound-only client** — the TRAILS client never opens a listening socket. All communication is initiated by the client as an outbound WebSocket connection. Zero inbound attack surface.

---

## A0.2 — Microcontroller Support (ESP32/ESP8266)

TRAILS reaches embedded devices. Any device that can open an outbound WebSocket connection over WiFi/Ethernet can be a node in the tree.

### ESP32 Client Footprint

| Capability | ESP32 library | Approximate size |
|---|---|---|
| WebSocket client | esp-websocket-client (ESP-IDF built-in) | ~15 KB |
| JSON | cJSON (ESP-IDF built-in) | ~10 KB |
| Ed25519 | tweetnacl or libsodium-esp32 | ~8 KB |
| Base64 | mbedtls (ESP-IDF built-in) | ~2 KB |
| **Total TRAILS client** | | **~35 KB** |

ESP32 has ~520 KB RAM. ESP8266 has ~80 KB. A stripped-down TRAILS client (open secLevel, no signing) could fit in ~20 KB.

### TRAILS_INFO Delivery on Embedded

Environment variables don't exist on bare-metal firmware. Alternatives:

- **NVS (Non-Volatile Storage)** — store TRAILS_INFO JSON in ESP32's NVS partition at provisioning time.
- **Hardcoded config** — compiled into firmware for fixed deployments.
- **MQTT bootstrap** — receive TRAILS_INFO via MQTT from a gateway, then connect to TRAILS server directly.
- **BLE provisioning** — phone app sends TRAILS_INFO via BLE during device setup.

All use `trails_init_with(config)` instead of `trails_init()`.

### Example: ESP32 Firmware

```c
#include "trails.h"

void app_main() {
    trails_handle h = trails_init_with(config_from_nvs());

    while (1) {
        float temp = read_sensor();
        char buf[128];
        snprintf(buf, sizeof(buf), "{\"temp\": %.1f}", temp);
        trails_status(h, buf);

        char *msg = trails_recv_timeout(h, 0);  /* non-blocking */
        if (msg) {
            /* handle reconfig, ota_update, cancel, etc. */
            trails_free_string(msg);
        }

        vTaskDelay(pdMS_TO_TICKS(10000));
    }
}
```

### IoT Tree Example

```
Cloud Orchestrator (K8s)
├── Region Controller US (K8s pod)
│   ├── Gateway (Raspberry Pi)
│   │   ├── Sensor Node A (ESP32)
│   │   ├── Sensor Node B (ESP32)
│   │   └── Actuator Node C (ESP8266)
│   └── Edge Processor (Jetson Nano)
└── Region Controller EU (K8s pod)
    └── ...
```

Every node — K8s pod to ESP32 — participates in the same tree. `trails tree` from a laptop shows the entire fleet. `trails cancel --cascade` reaches every device.

### Updated Client Platform Matrix

| Client | Platform | RAM required | Listening ports |
|---|---|---|---|
| client-rust | Servers, K8s | Standard | None |
| client-python | Data pipelines, ML | Standard | None |
| client-go | Microservices | Standard | None |
| client-java | Enterprise, Spark | Standard | None |
| client-c | ESP32, ESP8266, embedded, HPC | ~35 KB (full) / ~20 KB (minimal) | None |
| client-cli | Bash, terminal | Standard | None |
| client-kotlin | Android | Standard | None |
| client-swift | iOS | Standard | None |

---

## A0.3 — Bootstrap Provisioning (Burned-In Minimal Config)

### The Problem

A device (ESP32, Raspberry Pi, or even a pre-built container image) being manufactured or packaged doesn't know:

- Which parent will own it (could be sold/assigned to different customers)
- Which TRAILS server it will connect to (customer's infrastructure)
- What its appId will be (assigned at deployment time)
- What roleRefs apply (customer's RBAC policy)

But you **can** burn in the minimum needed to establish first contact.

### TRAILS_INFO Bootstrap Mode

The `TRAILS_INFO` envelope gains a `mode` field:

```json
// Full mode (K8s, VMs, processes — current spec)
{
  "v": 1,
  "mode": "full",
  "appId": "...",
  "parentId": "...",
  "appName": "...",
  "serverEp": "...",
  "serverPubKey": "...",
  "secLevel": "signed",
  ...all fields...
}

// Bootstrap mode (burned into firmware / baked into image)
{
  "v": 1,
  "mode": "bootstrap",
  "parentPubKey": "ed25519:AAAA...",
  "serverEp": "wss://factory-trails.company.com:8443/ws"
}
```

Two fields. The device knows who its parent is (by public key) and where to make first contact.

### Bootstrap Registration

Device powers on, generates ephemeral keypair, connects outbound, sends:

```json
{
  "type": "bootstrap_register",
  "child_pub_key": "ed25519:BBBB...",
  "device_info": {
    "chip": "ESP32-S3",
    "mac": "AA:BB:CC:DD:EE:FF",
    "firmware": "v1.0.3",
    "flash_id": "..."
  }
}
```

The server accepts this only if a parent holding the matching private key for `parentPubKey` has previously registered intent. The parent authenticates to trailsd by signing with its private key, proving it owns the `parentPubKey` burned into the device.

### Parent Claims the Device

```
POST /api/v1/claim
Authorization: <signed with parent's private key>
{
  "parentId": "aaa-...",
  "childMac": "AA:BB:CC:DD:EE:FF",
  "appId": "bbb-...",
  "appName": "sensor-floor-3-room-7",
  "roleRefs": ["factory-monitoring"],
  "newConfig": {
    "serverEp": "wss://customer-trails.bank.com:8443/ws",
    "parentPubKey": "ed25519:CCCC...",
    "serverPubKey": "ed25519:DDDD..."
  }
}
```

The TRAILS server delivers a `provision` control command to the device:

```json
{
  "type": "control",
  "action": "provision",
  "payload": {
    "appId": "bbb-...",
    "parentId": "aaa-...",
    "appName": "sensor-floor-3-room-7",
    "serverEp": "wss://customer-trails.bank.com:8443/ws",
    "parentPubKey": "ed25519:CCCC...",
    "serverPubKey": "ed25519:DDDD...",
    "secLevel": "signed",
    "roleRefs": ["factory-monitoring"]
  },
  "sig": "ed25519:..."
}
```

The device:

1. Verifies the signature against the factory server's key
2. Stores the new config in NVS (non-volatile storage) or equivalent persistent storage
3. Disconnects from factory TRAILS server
4. Reconnects to `customer-trails.bank.com` with its new identity
5. Sends a full `register` with the assigned `appId`

The device has been **re-parented** — transferred from factory to customer, with new server, new parent pubkey, new identity. All via the control path. No physical access needed after initial flash.

### Full Lifecycle

```
Manufacturing:
    Flash firmware with: parentPubKey(factory) + serverEp(factory)
    Device sits in warehouse.

Deployment:
    Device powers on → connects to factory TRAILS server
    Server records: "bootstrap device MAC:AA:BB:CC registered, awaiting claim"

Customer claims:
    Customer's orchestrator calls POST /api/v1/claim
    Factory TRAILS delivers "provision" command to device
    Device stores new config, reconnects to customer's TRAILS server

Operational:
    Device is now part of customer's tree
    Normal TRAILS operations: status, result, control, cancel

Re-provisioning (sold to different customer, reassigned to different fleet):
    Current parent sends: {"action": "provision", "payload": {new config}}
    Device stores new config, reconnects to new server
    Seamlessly transferred. No firmware reflash. No physical access.
```

### Key Rotation and Server Migration Via Control Path

After deployment, the parent can rotate keys without firmware updates:

```json
{
  "action": "rotate_keys",
  "payload": {
    "newParentPubKey": "ed25519:EEEE...",
    "newServerPubKey": "ed25519:FFFF...",
    "effectiveAfter": "2025-03-01T00:00:00Z"
  }
}
```

Device stores new keys, continues using old ones until effective date, then switches. No downtime.

Server URL migration:

```json
{
  "action": "migrate",
  "payload": {
    "newServerEp": "wss://new-trails.bank.com:8443/ws",
    "newServerPubKey": "ed25519:GGGG..."
  }
}
```

Device gracefully disconnects, reconnects to the new server. Entire fleet migration without touching a single device.

### Trust Chain

```
Manufacturing:
    Factory burns parentPubKey → device trusts "whoever holds this private key"

First contact:
    Device connects to factory server
    Parent proves identity by signing with matching private key
    → Trust established

Provisioning:
    Parent sends new config via "provision" command, signed by factory server
    Device verifies signature against known server key
    → New trust anchor installed

Operational:
    Device trusts new parentPubKey and new serverPubKey
    → Chain complete, factory no longer in the loop

Re-provisioning:
    Current parent sends new "provision" command
    → Trust anchor replaced, device moves to new owner
```

### Applicability Beyond Embedded

This pattern works for any pre-built artifact:

- **Container images** — bake bootstrap TRAILS_INFO into a Docker image at build time. At runtime, the orchestrator claims the container and delivers full config.
- **VM images (AMI, qcow2)** — bake bootstrap config into the image. On first boot, VM connects to provisioning TRAILS server, gets claimed, redirected to operational server.
- **Mobile app builds** — ship the app with factory TRAILS config. On first launch, the backend claims the app instance and provisions it with the right tree.
- **Desktop agent installers** — installer bakes in bootstrap config. On first run, agent connects, gets claimed by the enterprise's TRAILS server.

### New Control Actions

| Action | Purpose |
|---|---|
| `provision` | Deliver full TRAILS_INFO config to a bootstrapped device. Replaces server, parent, identity. |
| `rotate_keys` | Replace parentPubKey and/or serverPubKey with new keys. Supports effective date for zero-downtime rotation. |
| `migrate` | Change serverEp. Device disconnects from current server and reconnects to new one. |

### Spec Integration Notes

- §5 (TRAILS_INFO Envelope): Add `mode` field ("full" or "bootstrap") and document bootstrap schema.
- §8 (Wire Protocol): Add `bootstrap_register` message type.
- §11 (Open Command Vocabulary): Add `provision`, `rotate_keys`, `migrate` as system-level control actions (distinct from application-defined actions).
- §23 (REST API): Add `POST /api/v1/claim` endpoint.
- §3 (Core Design Principles): Note that TRAILS_INFO can be burned into images for deferred provisioning.

---

## A0.4 — Device Authorization and Lifecycle via Tree Relationships

### The Insight

The parent-child tree with re-parenting, revocable grants, and cascading cancellation isn't just a process management mechanism — it's a **device authorization model**. No new protocol mechanisms are needed. Everything already in the spec applies directly.

### The Problem with Static Pairing

Today's BLE/WiFi remotes and smart devices use static pairing. Once paired, forever paired. Revocation requires factory reset, manual unpairing, or hoping the manufacturer's cloud app works.

```
Old model (static pairing):
    Remote ←—BLE pair—→ TV
    Works forever until manually unpaired.
    Lost remote? Anyone who finds it controls your device.

TRAILS model (tree relationship):
    Remote (child) ——→ trailsd ——→ TV (parent)
    Parent controls the relationship.
    Lost remote? Parent revokes. Instant. Remote becomes inert.
```

### Example: Smart Door Locks

```
Homeowner's phone (parent)
├── Front door lock (child, device)
├── Guest keypad (child, device)
│   └── Guest's phone (grandchild, temporary)
└── Cleaning service fob (child, expiresAt: "Friday 17:00")
```

- Cleaning service fob expires Friday 5:01pm — relationship dies, fob is inert. No firmware update on the lock. No cloud revocation call.
- Guest leaves? `trails cancel --uuid $GUEST_KEYPAD_ID` — cascade kills guest's phone access too.
- Lost a fob? Cancel that one child. Other fobs keep working.

### Example: Car Key Fobs

```
Car ECU (parent, trailsd embedded)
├── Owner key fob A (child)
├── Owner key fob B (child)
├── Valet key fob (child, roles: ["start"], no "trunk", expiresAt: 4 hours)
└── Dealer diagnostic tool (child, roleRef: "service-diagnostics")
```

- Sell the car? New owner sends `provision` command — old owner's fobs instantly lose authority. No dealer visit.
- Key fob stolen? `trails cancel --uuid $STOLEN_FOB_ID` — that fob is dead, others keep working.
- Valet parking? Time-bounded child with scoped roles — can start the engine, can't open the trunk, expires in 4 hours.

### Example: Smart Home Remotes

```
TV (parent, trailsd on SoC)
├── Owner's remote (child, roles: ["*"])
├── Kid's remote (child, roles: ["volume", "channel"], no "settings", no "purchase")
└── Airbnb guest remote (child, expiresAt: "checkout Sunday 12:00")
```

- Re-purpose remote for different TV? Current TV parent sends `provision` with new TV's config. Remote seamlessly transfers.
- Old remote can't work on new owner's TV — the tree relationship to old owner is gone.

### The Device Lifecycle as TRAILS Operations

| Operation | TRAILS equivalent | New code needed? |
|---|---|---|
| Provision (pair device) | `provision` control command (A0.3) | No |
| Authorize | roleRefs with roles | No |
| Scope permissions | Specific roles per child | No |
| Time-bound access | Grant with `expiresAt` | No |
| Revoke one device | `cancel` that child | No |
| Revoke all (sell/transfer) | `provision` with new parent, or `cancel --cascade` | No |
| Re-purpose device | `provision` with new parent config | No |
| Audit (who unlocked at 3am?) | audit_log in Postgres with OAuth identity | No |

**Zero additional protocol mechanisms.** Every operation maps to existing TRAILS primitives.

### What Existing IoT Auth Lacks

| Capability | Zigbee/Z-Wave/Matter | TRAILS |
|---|---|---|
| Tree depth | Hub → device (1 level) | Arbitrary depth |
| Cascading revocation | No | Yes — cancel propagates through subtree |
| Cross-protocol | No (protocol-specific pairing) | Yes — transport-agnostic |
| Temporal grants | Binary (paired or not) | `expiresAt`, time-bounded roles |
| Scoped permissions | All-or-nothing | Per-child roles (volume yes, settings no) |
| Audit trail | Device-local, limited storage | Postgres, permanent, OAuth identity |
| Re-parenting | Factory reset + re-pair | One `provision` command, no reset |

### Applicability

This pattern applies wherever devices need authorized, scopable, revocable, time-bounded relationships:

- Smart home (locks, TVs, appliances, HVAC)
- Automotive (key fobs, valet keys, dealer diagnostics, fleet management)
- Enterprise (badge readers, printers, conference room systems)
- Healthcare (medical device access, patient monitor delegation)
- Industrial (machinery access, operator authorization, shift-based permissions)
- Hospitality (hotel room keys, Airbnb guest access, co-working space entry)

### Spec Integration Notes

No changes needed to the protocol or schema. This section documents a recognized application domain that validates the existing design. The only future additions might be:

- Example `client-c` code for ESP32 acting as a smart lock / remote
- Best practices guide for device lifecycle management using TRAILS
- Reference architecture for TRAILS as IoT device authorization layer

---

## A0.5 — Lightweight Encryption Without TLS (X25519 + ChaCha20-Poly1305)

### The Problem

mTLS on constrained devices is expensive:

| Component | mTLS (mbedTLS) | TRAILS crypto (tweetnacl) |
|---|---|---|
| Library code | ~50–100 KB | ~8 KB |
| Session RAM | ~10–30 KB | ~256 bytes (key + nonce) |
| Certificate storage | ~2–4 KB | 0 (keys are 32 bytes each) |
| Handshake round trips | Multiple | 0 (ECDH computed locally) |
| Certificate chain validation | Yes | Not needed |
| **Total** | **~60–130 KB** | **~8 KB** |

On ESP32 (520 KB RAM), mTLS consumes 20–30% of memory. On ESP8266 (80 KB), often impossible.

### The Key Insight: Keys Are Already There

Every TRAILS client already holds Ed25519 keys for signing. Ed25519 keys are mathematically convertible to X25519 (Curve25519 ECDH) keys — this is a standardized operation (RFC 7748, libsodium's `crypto_sign_ed25519_pk_to_curve25519`).

So every TRAILS client already has:

```
Child's X25519 private key      (derived from Ed25519 private key)
Server's X25519 public key      (derived from server's Ed25519 public key)
```

Encryption adds **zero additional library dependencies.** tweetnacl (~8 KB, already present for signing) provides everything: `crypto_box` (X25519 + XSalsa20 + Poly1305), `crypto_sign` (Ed25519), `crypto_scalarmult` (raw ECDH).

### The Scheme

```
Plain ws:// connection (no TLS stack)

1. Derive shared secret via X25519 ECDH:
   shared = X25519(child_private, server_public)

2. Derive symmetric key via HKDF:
   key = HKDF-SHA256(shared, salt=appId, info="trails-v1")

3. Encrypt each message with ChaCha20-Poly1305:
   nonce = seq (monotonic sequence number, 12 bytes)
   ciphertext = ChaCha20-Poly1305(key, nonce, plaintext)

4. Send over plain WebSocket:
   [8-byte seq][ciphertext][16-byte auth tag]
```

No handshake. No certificate exchange. No round trips. The shared secret exists the moment both sides have each other's public keys — which happens at registration.

### Strictly Better Than mTLS for Device-to-Server

```
mTLS:
    ESP32 ══TLS══▶ Load Balancer (PLAINTEXT HERE) ──▶ TRAILS server

TRAILS encrypted:
    ESP32 ──ws://──▶ Load Balancer (still encrypted) ──▶ TRAILS server (decrypts)
```

mTLS encrypts the transport — any TLS termination point (load balancer, CDN, reverse proxy) sees plaintext. TRAILS `encrypted` tier is **end-to-end** between device and daemon. Stronger security with less code.

### Forward Secrecy

Ephemeral ECDH per session. During registration, both sides contribute an ephemeral X25519 keypair:

```json
// Child sends:
{"type": "register", ..., "child_ephemeral": "x25519:EEEE...", "sig": "..."}

// Server responds:
{"type": "ack", "server_ephemeral": "x25519:FFFF...", "sig": "..."}
```

Session key derived from ephemeral ECDH. Ephemeral keys discarded after derivation. Past sessions can't be decrypted even if long-term keys are later compromised. Cost: one additional 32-byte key exchange at registration. Negligible.

### Revised Security Tiers

| Tier | Transport | Signing | Encryption | Memory cost | Use case |
|---|---|---|---|---|---|
| `open` | `ws://` | None | None | ~27 KB (no tweetnacl) | Dev, local |
| `signed` | `ws://` | Ed25519 | None | ~35 KB | Authenticity only |
| `encrypted` | `ws://` | Ed25519 | X25519+ChaCha20 | **~35 KB (same!)** | Constrained devices, full security |
| `full` | `wss://` | Ed25519 | TLS | ~95–165 KB | Regulated, standard infra |

Going from `signed` to `encrypted` costs zero additional memory. The crypto library is already loaded. The only addition is one 32-byte symmetric key in RAM.

### Developer Experience

Identical across all tiers. The library handles everything internally:

```c
// Same code whether secLevel is open, signed, encrypted, or full
trails_status(h, "{\"temp\": 23.5}");
char *msg = trails_recv_timeout(h, 0);
```

### Using tweetnacl

tweetnacl is a single C file (~700 lines, ~8 KB compiled), no dependencies, no memory allocation, runs on anything with a C compiler:

```c
// Encrypt (sender):
crypto_box(ciphertext, plaintext, len, nonce, server_pk, child_sk);

// Decrypt (receiver):
crypto_box_open(plaintext, ciphertext, len, nonce, child_pk, server_sk);
```

Two function calls. That's the entire encryption layer.

### Spec Integration Notes

- §8 (Wire Protocol): Add `encrypted` secLevel tier, document message framing for encrypted payloads.
- §15 (Security Model): Add X25519 ECDH key derivation, forward secrecy via ephemeral keys.
- §5 (TRAILS_INFO): `secLevel` gains "encrypted" as fourth option.

---

## A0.6 — TRAILS as MQTT Replacement for IoT

### The Realization

With WebSocket + ECDH encryption + bidirectional control, TRAILS provides everything MQTT provides — and more — with less memory.

### What MQTT Gives

- Publish/subscribe messaging
- QoS levels (0: fire-and-forget, 1: at-least-once, 2: exactly-once)
- Retained messages (last known value)
- Last Will and Testament (crash notification)
- Topic-based routing
- Bidirectional communication

### What MQTT Costs on Constrained Devices

| Component | Typical size |
|---|---|
| MQTT client library (e.g., Eclipse Paho) | ~30–40 KB |
| TLS stack for MQTTS (mbedTLS) | ~50–100 KB |
| Certificate storage | ~2–4 KB |
| Session state (subscriptions, QoS queues) | ~5–10 KB |
| **Total** | **~90–150 KB** |

AWS IoT Core requires mTLS. Azure IoT Hub requires TLS + SAS tokens. Neither works without the full TLS stack. On ESP8266 with 80 KB RAM, this leaves almost nothing for the application.

### What TRAILS Gives (Already)

Every MQTT capability maps to an existing TRAILS feature:

| MQTT feature | TRAILS equivalent | Notes |
|---|---|---|
| Publish (device → cloud) | `trails_status()`, `trails_result()` | Structured JSON, not raw bytes |
| Subscribe (cloud → device) | `on()` handler registration | Type-safe, per-action handlers |
| QoS 0 (fire-and-forget) | Default message delivery | Same semantics |
| QoS 1 (at-least-once) | Server ack with seq number | Already in protocol |
| QoS 2 (exactly-once) | Seq + dedup on server (if needed) | Postgres dedup by app_id + seq |
| Retained messages | Latest snapshot in Postgres | `GET /apps/{id}/snapshots/latest` |
| Last Will / Testament | Crash detection (connection drop) | Already in protocol, richer (crash_type, context) |
| Topic hierarchy | Parent-child tree | Stronger — enforced authorization, not just naming convention |
| Bidirectional | Control path (parent → child) | Open vocabulary, acknowledged responses |

### What TRAILS Gives That MQTT Doesn't

| Capability | MQTT | TRAILS |
|---|---|---|
| Process/device tree | No (flat topic space) | Full parent-child tree in Postgres |
| Cascading cancellation | No | `cancel --cascade` across entire subtree |
| Structured results | No (raw bytes) | Typed JSON with business semantics |
| Command acknowledgment | No (pub/sub is fire-and-forget) | Every command gets ack + response |
| Authorization by relationship | No (ACL-based) | Tree-scoped — parent controls children |
| Device re-parenting | No | `provision` command, instant transfer |
| Temporal grants | No | `expiresAt` on grants and roleRefs |
| Audit trail | No | Full history in Postgres with OAuth identity |
| Bootstrap provisioning | No (requires pre-provisioning) | Burn minimal config, claim later (A0.3) |
| Persistent queryable history | No (broker is transient) | Postgres — query any device's full history |

### The Memory Comparison

| Stack | Library | TLS | Session state | Total |
|---|---|---|---|---|
| MQTT + mTLS (AWS IoT) | ~35 KB | ~60–100 KB | ~10 KB | **~105–145 KB** |
| MQTT + mTLS (Azure IoT) | ~35 KB | ~60–100 KB | ~10 KB | **~105–145 KB** |
| MQTT + no TLS (insecure) | ~35 KB | 0 | ~10 KB | **~45 KB** |
| **TRAILS encrypted** | **~15 KB (ws)** | **0** | **~8 KB (tweetnacl)** | **~35 KB** |

TRAILS `encrypted` tier: **3–4× less memory** than MQTT+mTLS, with **stronger security** (end-to-end vs TLS-terminated) and **richer features** (tree, cascade, RBAC, audit).

Even compared to insecure MQTT (no TLS), TRAILS is smaller AND encrypted.

### What Dies: The MQTT Broker

MQTT requires a broker (Mosquitto, EMQX, AWS IoT Core, Azure IoT Hub). The broker is:

- Another piece of infrastructure to deploy, secure, monitor, and scale
- A flat topic-based router with no understanding of device relationships
- A separate system from your application database (device state is split between broker and DB)
- A cost center (AWS IoT Core charges per million messages; Azure IoT Hub charges per unit)

TRAILS replaces the broker with trailsd + Postgres. Device state, message history, tree structure, authorization — all in one place. One system to deploy, one to query, one to secure.

### The Architecture Comparison

```
AWS IoT today:
    ESP32 ══mTLS══▶ AWS IoT Core (MQTT broker)
                        │
                        ├──▶ IoT Rules Engine ──▶ Lambda ──▶ DynamoDB
                        ├──▶ IoT Shadow (device state)
                        └──▶ IoT Device Defender (security)
    
    5 services. Complex. Expensive. 100+ KB on device.

TRAILS:
    ESP32 ──ws://──▶ trailsd ──▶ Postgres
    
    1 service. Device state, messages, tree, auth, audit — all in Postgres.
    35 KB on device.
```

### Migration Path for Existing MQTT Deployments

TRAILS doesn't require ripping out MQTT overnight. A bridge pattern:

```
Phase 1: MQTT bridge
    ESP32 ──MQTT──▶ Mosquitto ──bridge──▶ trailsd
    Existing devices unchanged. trailsd receives via bridge.

Phase 2: New devices use TRAILS directly
    New ESP32 ──ws://──▶ trailsd
    Old devices still via bridge.

Phase 3: Firmware update for old devices
    All ESP32 ──ws://──▶ trailsd
    MQTT broker decommissioned.
```

### When MQTT Is Still Better (Honestly, Very Few Cases)

- **UDP-based variants (MQTT-SN)** — for extremely constrained networks (6LoWPAN, Thread) where TCP/WebSocket is too heavy. TRAILS requires TCP.
- **Existing ecosystem** — millions of deployed MQTT devices, extensive tooling, cloud provider integration. TRAILS is new.
- **Simple sensor telemetry with no relationships** — if devices are truly flat (no tree, no parent-child, no authorization needed) and you just need raw telemetry ingestion, MQTT's simplicity wins.

Note: MQTT's "massive fan-out pub/sub" is often cited as an advantage, but in practice MQTT brokers don't scale well for this. See A0.7 for how TRAILS handles fan-out via Kafka/NATS, which is strictly more scalable than any MQTT broker.

### Spec Integration Notes

- §28 (Comparison): Add MQTT comparison table.
- §3 (Core Design Principles): Note that TRAILS can serve as IoT messaging layer, replacing MQTT+TLS with WebSocket+ECDH.
- Examples: Add `examples/esp32-mqtt-migration/` showing side-by-side MQTT vs TRAILS code.

---

## A0.7 — Fan-Out via Kafka/NATS (Replacing MQTT's Last Claimed Advantage)

### The MQTT Scalability Myth

MQTT pub/sub is often cited as its key advantage: "100,000 subscribers to one topic." In practice:

- **Mosquitto** — single-threaded, practical limit ~100K concurrent connections on a beefy server. Fan-out to 100K subscribers on a single topic causes latency spikes and memory pressure.
- **EMQX / HiveMQ** — clustered, better, but still limited by broker-to-subscriber delivery. Every message is copied per subscriber in broker memory. 1 message × 100K subscribers = 100K copies in RAM before delivery.
- **AWS IoT Core** — throttled. Default: 500 publish requests/sec per account. Fan-out via IoT Rules → Lambda → downstream. Not real-time pub/sub at scale.

The fundamental problem: **MQTT brokers are the bottleneck.** The broker receives one message and must deliver it to N subscribers. This is an O(N) operation in broker memory and network I/O. The broker is doing the fan-out, and it wasn't built for massive scale.

### The TRAILS Architecture: Separate Ingestion from Fan-Out

TRAILS has a natural separation that MQTT lacks:

```
MQTT architecture:
    Device ──▶ MQTT Broker ──▶ Subscriber 1
                           ──▶ Subscriber 2
                           ──▶ ...
                           ──▶ Subscriber N
    
    Broker does EVERYTHING: receive, store, route, deliver, fan-out.
    Single bottleneck.

TRAILS architecture:
    Device ──ws://──▶ trailsd ──▶ Postgres (durable store)
                             ──▶ Internal event bus
                                    │
                                    ├──▶ Kafka / NATS (fan-out)
                                    │       │
                                    │       ├──▶ Consumer group A (analytics)
                                    │       ├──▶ Consumer group B (alerting)
                                    │       ├──▶ Consumer group C (dashboards)
                                    │       └──▶ Consumer group N (any subscriber)
                                    │
                                    └──▶ Parent routing (tree-scoped, existing)
```

trailsd receives and stores. Kafka/NATS handles fan-out. Each is purpose-built for its job.

### Why This Is Strictly Better

**Kafka** is built for exactly this problem. One message written to a topic partition, consumed by N consumer groups independently. No message copying per subscriber. Consumers pull at their own pace. Retention for days/weeks. Replay from any offset. Millions of messages/sec throughput.

**NATS JetStream** similarly — publish once, consume by N subscribers. Push or pull. Replay. Clustering. Millions of msg/sec.

Both are proven at scales MQTT brokers can't approach:

| System | Proven scale | Fan-out model |
|---|---|---|
| Mosquitto | ~100K connections | Broker copies per subscriber |
| EMQX cluster | ~millions connections | Broker copies, clustered |
| AWS IoT Core | Throttled (500 pub/sec default) | Rules engine, not real-time |
| **Kafka** | **Millions msg/sec, petabytes** | **Write-once, read-many (zero-copy)** |
| **NATS JetStream** | **Millions msg/sec** | **Publish-once, consume-many** |

### The Integration Point

trailsd already has an internal event bus (`tokio::sync::broadcast`). Adding a Kafka/NATS producer is a few lines of configuration:

```yaml
# trailsd config
fan_out:
  enabled: true
  backend: "kafka"          # or "nats"
  kafka:
    brokers: ["kafka-1:9092", "kafka-2:9092"]
    topic_pattern: "trails.{namespace}.{msg_type}"
  nats:
    url: "nats://nats:4222"
    subject_pattern: "trails.{namespace}.{msg_type}"
```

When a message arrives from a device:

1. Store in Postgres (durable record of truth) — already happens
2. Route to parent (tree-scoped delivery) — already happens
3. Publish to Kafka/NATS topic (fan-out to observers) — new, optional

Step 3 is fire-and-forget from trailsd's perspective. Kafka/NATS handles delivery to all subscribers. trailsd doesn't know or care how many subscribers exist.

### Topic Mapping

TRAILS events naturally map to Kafka/NATS topics:

```
trails.datascan.status          ← all status updates from namespace "datascan"
trails.datascan.result          ← all results
trails.datascan.error           ← all errors
trails.datascan.crash           ← all crashes
trails.datascan.control         ← all control commands (audit)
trails.*.crash                  ← all crashes across all namespaces
trails.*.result                 ← all results everywhere
```

Consumers subscribe to the topics they care about:

- Analytics pipeline subscribes to `trails.*.result`
- PagerDuty integration subscribes to `trails.*.crash` and `trails.*.error`
- Grafana dashboard subscribes to `trails.datascan.status`
- Compliance recorder subscribes to `trails.compliance.*`
- ML training monitor subscribes to `trails.gpu-pool.status`

Each consumer group processes independently. Adding a new subscriber doesn't affect existing ones. No trailsd changes needed.

### The Device Doesn't Know

This is the critical point. The ESP32 sends:

```c
trails_status(h, "{\"temp\": 23.5}");
```

It doesn't know that this message:

1. Gets stored in Postgres
2. Gets routed to its parent
3. Gets published to Kafka
4. Gets consumed by 5 different analytics pipelines
5. Gets consumed by an alerting system
6. Gets consumed by a real-time dashboard

The device's code, memory footprint, and network usage are identical regardless of how many downstream consumers exist. Fan-out is the server's problem, and the server delegates it to infrastructure built for scale.

### The Corrected Comparison

| Capability | MQTT broker | TRAILS + Kafka/NATS |
|---|---|---|
| Fan-out to 100 subscribers | Broker copies × 100 | Kafka/NATS zero-copy |
| Fan-out to 100K subscribers | Broker struggles | Kafka/NATS native scale |
| Subscriber independence | All share broker resources | Consumer groups are isolated |
| Replay/rewind | Limited (retained messages) | Full replay from any offset |
| Retention | Broker memory (limited) | Kafka: days/weeks on disk. Plus Postgres forever. |
| Throughput | 10K–100K msg/sec (broker-bound) | Millions msg/sec |
| Device impact of adding subscribers | None | None |
| Device code changes | None | None |

### What This Means for the "When MQTT Is Better" List

The massive fan-out pub/sub argument — MQTT's supposed strongest advantage — is actually MQTT's **weakness** at scale. MQTT brokers are the bottleneck. TRAILS delegates fan-out to Kafka/NATS, which are purpose-built for it and proven at orders of magnitude greater scale.

The only remaining MQTT advantages are:

- MQTT-SN over UDP for extremely constrained networks
- Existing ecosystem and device fleet
- Raw simplicity for truly flat, relationship-free telemetry

### Spec Integration Notes

- §20 (Observer Model): Kafka/NATS as the fan-out backend for the observer tier.
- §21 (Server Architecture): Add optional Kafka/NATS producer in the server pipeline.
- §29 (Phase Plan): Kafka/NATS integration as Phase 6 alongside observer model.

---

## A0.8 — (Reserved for Future Additions)

*Collecting low-hanging fruits. This section will be populated as design insights emerge without complicating the core codebase.*

---

*Each addendum is numbered A0, A1, A2, ... and references specific sections of TRAILS-SPEC.md where the additions should be integrated when the spec is next revised.*