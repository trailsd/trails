# TRAILS — Addendum 1: IoT Location-Aware Device Trees

**Addendum to:** TRAILS-SPEC.md v2.0
**Date:** 2026-02-26
**Status:** Design notes

---

## The Insight

Every IoT device exists in a physical location. Today, location is metadata — a label slapped onto a device record in a cloud dashboard. With TRAILS, location **is the tree structure itself.** The parent-child hierarchy naturally mirrors the physical world.

## The Tree Is the World

```
OEM Cloud (root trailsd)
├── Country: India (UUID, trailsd: trails.in.oem.com)
│   ├── City: Bengaluru (UUID)
│   │   ├── Location: Whitefield (UUID)
│   │   │   ├── House: #42 MG Road (UUID, trailsd: home-minibox)
│   │   │   │   ├── Room: Living Room (UUID)
│   │   │   │   │   ├── Smart TV (ESP32, UUID)
│   │   │   │   │   ├── AC Controller (ESP8266, UUID)
│   │   │   │   │   └── Light Panel (ESP32, UUID)
│   │   │   │   ├── Room: Kitchen (UUID)
│   │   │   │   │   ├── Fridge Monitor (ESP32, UUID)
│   │   │   │   │   └── Gas Detector (ESP8266, UUID)
│   │   │   │   └── Room: Garage (UUID)
│   │   │   │       └── Door Lock (ESP32, UUID)
│   │   │   │
│   │   │   └── House: #43 MG Road (UUID, trailsd: home-minibox)
│   │   │       └── ...
│   │   │
│   │   └── Location: Koramangala (UUID)
│   │       └── ...
│   │
│   └── City: Mumbai (UUID)
│       └── ...
│
├── Country: USA (UUID, trailsd: trails.us.oem.com)
│   └── ...
│
└── Country: Japan (UUID, trailsd: trails.jp.oem.com)
    └── ...
```

Every level is a UUID. Every level is a TRAILS node. The tree **is** the topology.

## What You Get for Free

### 1. Location-Scoped Operations

```bash
# Turn off all lights in the living room
trails send $LIVING_ROOM_UUID reconfig --payload '{"power": "off"}' --cascade

# Cancel all devices in house #42 (family going on vacation)
trails send $HOUSE_42_UUID sleep_mode --cascade

# Emergency: shut down all gas detectors in Whitefield for firmware recall
trails send $WHITEFIELD_UUID firmware_recall --cascade \
    --filter '{"device_type": "gas_detector"}'

# What's the status of every device in Bengaluru?
trails tree --uuid $BENGALURU_UUID
```

Cascade follows the tree. "Turn off living room" sends to every child of the living room node. "Shut down the house" sends to every room, every device. "Recall in Whitefield" reaches every house, every room, every matching device. One command, tree-scoped, no device enumeration needed.

### 2. Local-First with Cloud Sync

The home-minibox (a Raspberry Pi or similar) runs its own trailsd:

```
House #42's minibox (local trailsd + local Postgres):
    ├── Living Room devices connect locally (LAN, <1ms latency)
    ├── Kitchen devices connect locally
    └── Garage devices connect locally

    minibox itself is a child of City/Location node in cloud trailsd
```

Devices talk to the local trailsd over WiFi/LAN. The minibox talks to the cloud trailsd over internet. If internet goes down, the house still works — local control, local state, local Postgres. When internet returns, the minibox syncs upstream.

This is the **edge computing** pattern that every IoT platform tries to bolt on. In TRAILS, it falls out naturally from the tree. The minibox is just another node — a parent to house devices, a child of the location node.

### 3. Device Discovery via Tree Walk

No mDNS, no UPnP, no SSDP, no device scanning. You know what's in a room because the room's children are the devices:

```bash
# What's in the living room?
trails children --uuid $LIVING_ROOM_UUID

# Response:
# smart-tv      (aa11-...) [connected] ESP32 firmware:v2.1
# ac-controller (bb22-...) [connected] ESP8266 firmware:v1.8
# light-panel   (cc33-...) [connected] ESP32 firmware:v3.0
```

Move a device to a different room? Re-parent it:

```json
{"action": "provision", "payload": {"parentId": "$KITCHEN_UUID"}}
```

The light panel is now in the kitchen. No re-pairing, no factory reset, no re-discovery. One command.

### 4. OEM Provisioning Flow

At the factory, every device is burned with:

```json
{
  "v": 1,
  "mode": "bootstrap",
  "parentPubKey": "ed25519:OEM_KEY...",
  "serverEp": "wss://factory.oem.com:8443/ws"
}
```

Customer buys device, powers it on at home. The home-minibox (or phone app) claims it:

```
POST /api/v1/claim
{
  "childMac": "AA:BB:CC:DD:EE:FF",
  "appId": "new-uuid-for-this-device",
  "appName": "living-room-light",
  "parentId": "$LIVING_ROOM_UUID",
  "newConfig": {
    "serverEp": "ws://192.168.1.100:8443/ws",
    "parentPubKey": "ed25519:HOME_MINIBOX_KEY..."
  }
}
```

Device disconnects from OEM factory server, reconnects to local minibox. It's now part of the house tree. OEM is out of the loop. Customer owns the device fully.

### 5. Multi-Tenant Buildings

An apartment complex or office building:

```
Building: Prestige Tower (UUID, trailsd: building server)
├── Floor 1 (UUID)
│   ├── Apt 101 (UUID, tenant: alice@..., roleRefs: ["tenant-full"])
│   │   ├── Thermostat (UUID)
│   │   └── Door Lock (UUID)
│   └── Apt 102 (UUID, tenant: bob@..., roleRefs: ["tenant-full"])
│       └── ...
├── Floor 2 (UUID)
│   └── ...
├── Common: Lobby (UUID, roleRefs: ["building-admin", "security-team"])
│   ├── CCTV Controller (UUID)
│   └── Access Gate (UUID)
└── Common: Parking (UUID)
    └── Gate Controller (UUID)
```

Alice can control devices in Apt 101 (she's in the roleRef grant for that subtree). She cannot control Apt 102 or lobby devices. Building admin can control common areas. Security team has read access to CCTV. All enforced by the tree and roleRefs. No separate access control system.

### 6. Disaster Recovery / Relocation

Family moves from House #42 to a new house. The minibox moves with them:

```bash
# Old location parent re-parents minibox to new location
trails send $MINIBOX_UUID provision \
    --payload '{"parentId": "$NEW_HOUSE_UUID"}'
```

The minibox and all its children (every device in the house) are now under the new location node. The devices don't know or care — they talk to the minibox, which is still there on the local network. Only the minibox's parent changed.

If a device breaks and is replaced, the new device gets provisioned into the same spot in the tree with the same UUID (or a new UUID with the same parent and appName). The rest of the tree is unaffected.

### 7. The Query Power

Because the tree is in Postgres:

```sql
-- "How many devices are online in Bengaluru right now?"
WITH RECURSIVE subtree AS (
    SELECT app_id FROM apps WHERE app_id = :bengaluru_uuid
    UNION ALL
    SELECT a.app_id FROM apps a JOIN subtree s ON a.parent_id = s.app_id
)
SELECT count(*) FROM subtree s
JOIN apps a ON s.app_id = a.app_id
WHERE a.status = 'connected';

-- "Which rooms have offline devices?"
-- (room-level nodes whose children include disconnected devices)

-- "Average temperature across all houses in Whitefield"
-- (aggregate latest snapshots from all temp sensors under Whitefield subtree)

-- "Which devices haven't sent a status in 24 hours?"
-- (devices where latest snapshot is old — potential hardware failure)
```

## The Architecture

```
Cloud tier:
    OEM trailsd (root) ──▶ Postgres (global device registry)
        │
        ├── Country trailsd instances (regional)
        │
        └── Kafka/NATS fan-out for analytics, monitoring

Edge tier (per house/building):
    Home minibox trailsd ──▶ Local Postgres (local state)
        │
        ├── Room 1 devices (ESP32/ESP8266, ws:// to minibox)
        ├── Room 2 devices
        └── ...
    
    Minibox syncs upstream to city/country trailsd
    If internet down → house still works locally
    If internet returns → sync resumes

Device tier:
    ESP32/ESP8266, 35 KB TRAILS client
    ws:// to local minibox (LAN, <1ms)
    No internet required for local operation
    No cloud dependency for basic control
```

## What Existing IoT Platforms Require vs TRAILS

| Capability | AWS IoT / Azure IoT / Google IoT | TRAILS |
|---|---|---|
| Location hierarchy | Metadata labels (flat) | Tree structure (native) |
| Local operation without cloud | Greengrass / IoT Edge (complex add-on) | Natural — minibox is a trailsd node |
| Device-to-device within house | Via cloud round-trip (!) | Local — both talk to minibox |
| Cascade commands to a room | Custom Lambda/Rules per device | `trails send $ROOM --cascade` |
| Multi-tenant isolation | IAM policies (complex) | Tree + roleRefs (structural) |
| Device relocation | Manual deregister + re-register | One `provision` command |
| Device discovery | IoT registry + mDNS/UPnP | Tree children query |
| Offline resilience | Limited (Greengrass caching) | Full local trailsd + Postgres |
| Memory on device | 100+ KB (MQTT + mTLS) | 35 KB |

## Spec Integration Notes

- This addendum introduces the concept of **trailsd federation** — multiple trailsd instances forming a hierarchy (cloud → country → city → house → room → device). The federation protocol (how trailsd instances sync with each other) is future work but the tree structure supports it naturally.
- The home-minibox concept should be referenced in the deployment section as an edge deployment option alongside K8s DaemonSet and centralized Deployment.
- The `provision` command from A0.3 (Addendum 0) is the enabler for device relocation and tree restructuring.

---

*Addendum 1 documents IoT location-aware device trees as a natural application of the TRAILS parent-child hierarchy. No new protocol mechanisms are needed — tree structure, cascade operations, roleRefs, bootstrap provisioning, and the encrypted security tier provide everything.*