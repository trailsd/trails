# TRAILS — Addendum 2: Mobile Multi-Persona Identity (Keypair as Identity)

**Addendum to:** TRAILS-SPEC.md v2.0
**Date:** 2026-02-26
**Status:** Design notes

---

## The Problem

The spec defines two authorization paths:

- **Tree-internal:** Ed25519 keypair, automatic, parent controls children
- **External (non-parent):** OAuth/OIDC, enterprise identity

But a normal home has no OAuth server. A family doesn't run Keycloak. A car owner doesn't have an OIDC provider. Asking a grandmother to OAuth into her smart TV is absurd.

Yet the use cases are real:

- Mom's phone opens the car remotely for the kid who forgot the key
- Dad's phone stops the TV at 10pm (parental control)
- Guest's phone unlocks the front door for the weekend
- Teenager's phone can control their room lights but not the thermostat

These are **non-parent** actors controlling devices. The spec says they need OAuth. But homes don't have OAuth. We need a third identity path.

## The Insight: The Public Key IS the Identity

Every TRAILS client already generates an Ed25519 keypair. The public key is globally unique (256-bit random). It's unforgeable. It's verifiable. It doesn't need a central authority to issue it.

For consumer/home scenarios, **the public key itself is the identity.** No usernames. No passwords. No OAuth. No tokens. No accounts. The key proves who you are.

```
OAuth identity:   "alice@company.com"     — issued by IdP, requires infrastructure
Keypair identity: "ed25519:Ax7f9K2..."    — self-issued, requires nothing
```

## Mobile Multi-Persona

A single TRAILS mobile app holds multiple keypairs, each representing a different persona:

```
TRAILS App on Mom's Phone:
    ┌─────────────────────────────────────────────┐
    │  Persona: "Home"                            │
    │  Private key: [stored in Secure Enclave]    │
    │  Public key:  ed25519:AAAA...               │
    │  Grants: home lock, lights, TV, thermostat  │
    │                                             │
    │  Persona: "Car"                             │
    │  Private key: [stored in Secure Enclave]    │
    │  Public key:  ed25519:BBBB...               │
    │  Grants: car doors, engine start, trunk     │
    │                                             │
    │  Persona: "Office"                          │
    │  Private key: [stored in Secure Enclave]    │
    │  Public key:  ed25519:CCCC...               │
    │  Grants: office door, printer               │
    │                                             │
    │  Persona: "Parents' House"                  │
    │  Private key: [stored in Secure Enclave]    │
    │  Public key:  ed25519:DDDD...               │
    │  Grants: front door only, read-only cameras │
    └─────────────────────────────────────────────┘
```

Each persona is an independent keypair. Each has different grants on different device trees. The phone is not "one identity" — it's a **keyring** of personas, like carrying multiple physical keys.

## How Grants Work With Keypair Identity

When the home minibox (trailsd) is set up, the admin (whoever set up the house) registers public keys with grants:

```bash
# On the home minibox — admin registers family members

# Mom gets full control
trails grant add --pubkey "ed25519:AAAA..." \
    --name "Mom" \
    --scope $HOUSE_UUID \
    --roles "read,write,cancel" \
    --cascade   # applies to all children (rooms, devices)

# Teenager gets room-only control
trails grant add --pubkey "ed25519:XXXX..." \
    --name "Alex" \
    --scope $ALEX_ROOM_UUID \
    --roles "read,write"
    # No cascade to other rooms — only Alex's room

# Guest gets front door only, expires Sunday
trails grant add --pubkey "ed25519:YYYY..." \
    --name "Weekend Guest" \
    --scope $FRONT_DOOR_UUID \
    --roles "write" \
    --expires "2025-03-02T12:00:00Z"
```

## The Postgres Model

```sql
CREATE TABLE keypair_grants (
    id              BIGSERIAL PRIMARY KEY,
    pub_key         TEXT NOT NULL,           -- ed25519 public key (the identity)
    display_name    TEXT,                    -- "Mom", "Alex", "Weekend Guest"
    scope_app_id    UUID NOT NULL,           -- which subtree this grant applies to
    roles           TEXT[] NOT NULL,
    cascade         BOOLEAN DEFAULT false,   -- applies to children of scope?
    expires_at      TIMESTAMPTZ,
    created_by      TEXT NOT NULL,           -- pub_key of whoever created this grant
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_kp_grants_pubkey ON keypair_grants(pub_key);
CREATE INDEX idx_kp_grants_scope ON keypair_grants(scope_app_id);
CREATE INDEX idx_kp_grants_expiry ON keypair_grants(expires_at)
    WHERE expires_at IS NOT NULL;
```

## The Authorization Check

When Mom's phone sends a command:

```
POST /api/v1/apps/$TV_UUID/control
X-Trails-PubKey: ed25519:AAAA...
X-Trails-Sig: ed25519:<signature of request body>

{"action": "power_off"}
```

The server:

1. Verifies signature against the claimed public key (proves possession of private key)
2. Looks up `keypair_grants` for this public key
3. Checks: does any grant cover `$TV_UUID` (directly or via cascade from an ancestor)?
4. Checks: does the grant include `write` role?
5. Checks: is the grant expired?
6. Yes to all → deliver command to TV. Log in audit trail.

No OAuth. No token exchange. No HTTP round-trip to an IdP. Just Ed25519 verify (microseconds) + Postgres lookup (milliseconds).

## The Three Authorization Paths (Revised)

| Path | Identity | Authentication | Use case |
|---|---|---|---|
| Tree-internal | appId + parentId | Ed25519 signature (process keypair) | Process controls its children |
| Keypair-external | Public key | Ed25519 signature (persona keypair) | Consumer IoT, home, car, mobile |
| OAuth-external | OAuth subject | JWT Bearer token | Enterprise, multi-team, regulated |

The spec's two-domain model becomes three. But the third (keypair-external) uses the **same cryptographic primitive** as tree-internal — Ed25519. No new crypto code. The server just has an additional lookup: "is this public key granted access to this subtree?"

## Real Scenarios

### Open Car Remotely

```
Mom is at office. Kid calls: "I forgot my key, I'm locked out of the car."

Mom opens TRAILS app → selects "Car" persona → taps "Unlock doors"

Phone sends:
    POST /api/v1/apps/$CAR_DOORS_UUID/control
    X-Trails-PubKey: ed25519:BBBB...  (Car persona)
    X-Trails-Sig: ...
    {"action": "unlock", "payload": {"doors": "all"}}

Car's trailsd verifies:
    - Signature valid? Yes
    - pubkey BBBB has grant on $CAR_DOORS_UUID? Yes, role "write"
    - Expired? No
    → Deliver to car door controller
    → Doors unlock
    → Audit: "ed25519:BBBB (Mom/Car) unlocked all doors at 15:32"
```

### Stop TV at 10pm (Parental Control)

```
Dad's phone has a scheduled TRAILS command (cron in the app):

Every day at 22:00:
    POST /api/v1/apps/$TV_UUID/control
    X-Trails-PubKey: ed25519:AAAA...  (Home persona)
    {"action": "power_off", "payload": {"reason": "bedtime"}}

TV receives control command → powers off.
Kid can't override — they don't have "write" grant on the TV.
Kid's persona only has grants on their room devices.
```

### Guest Access

```
Airbnb host creates a temporary keypair grant:

trails grant add --pubkey "ed25519:GUEST_KEY..." \
    --name "Airbnb Guest Feb 25-28" \
    --scope $FRONT_DOOR_UUID --roles "write" \
    --expires "2025-02-28T11:00:00Z"

trails grant add --pubkey "ed25519:GUEST_KEY..." \
    --name "Airbnb Guest Feb 25-28" \
    --scope $LIVING_ROOM_UUID --roles "write" \
    --cascade --expires "2025-02-28T11:00:00Z"

Guest's phone can:
    ✓ Unlock front door
    ✓ Control living room lights, TV, AC
    ✗ Access bedroom (no grant)
    ✗ Access garage (no grant)
    ✗ Anything after Feb 28 11:00 (expired)
```

### Family Key Sharing

How does a guest get their keypair grant? Several options:

**QR Code:** Host's app generates a QR code containing the grant info. Guest scans with their TRAILS app. App generates a keypair, host's app registers the public key with the minibox.

**NFC Tap:** Host's phone taps guest's phone. Same exchange via NFC.

**Link/SMS:** Host sends a one-time link. Guest opens in TRAILS app. Keypair generated, public key registered.

**Manual:** Host types in guest's public key (displayed in guest's app as a short code like `AXFK-9R2M-...`).

In every case, the private key never leaves the guest's phone. Only the public key is shared with the minibox. The guest can't be impersonated even if the QR code or link is intercepted — the interceptor doesn't have the private key in the guest's Secure Enclave.

## Persona Lifecycle

### Creation

```
User opens TRAILS app → "Add Persona" → names it "Beach House"
App generates Ed25519 keypair in Secure Enclave / Keystore
Private key: never leaves the device, hardware-protected
Public key: displayed as short code or QR for registration
```

### Registration With a Device Tree

```
User visits beach house → scans minibox QR code (contains minibox trailsd URL)
App sends public key to minibox
Beach house admin approves → creates keypair_grant
Persona is now active for that device tree
```

### Revocation

```
# Admin revokes a persona's access
trails grant remove --pubkey "ed25519:YYYY..." --scope $HOUSE_UUID

# Or revoke all grants for a public key
trails grant remove --pubkey "ed25519:YYYY..." --all
```

Instant. The public key is no longer in `keypair_grants`. Next request from that phone is rejected. No firmware update on any device. No token expiration to wait for.

### Lost Phone

```
# Phone is lost/stolen. Admin revokes all that phone's persona keys:
trails grant remove --pubkey "ed25519:AAAA..." --all   # Home persona
trails grant remove --pubkey "ed25519:BBBB..." --all   # Car persona

# Every grant for those public keys is gone instantly.
# Thief has the phone but can't use any persona.
```

If the phone has biometric lock on the TRAILS app (Face ID / fingerprint required to sign with private key), the thief can't even attempt authentication. But revoking grants is the belt-and-suspenders protection.

### New Phone

```
User gets new phone → installs TRAILS app → creates new personas (new keypairs)
Goes to each device tree (home minibox, car, office) and registers new public keys
Old phone's keys are already revoked
```

This is exactly like getting new physical keys after losing your keyring. New keys, re-register with each lock. Simple mental model.

## Comparison: Keypair Identity vs Alternatives

| Aspect | Username/Password | OAuth/OIDC | TRAILS Keypair |
|---|---|---|---|
| Infrastructure needed | User DB | IdP server, token endpoint | None |
| Internet required | Yes (auth server) | Yes (token validation) | No (local Secure Enclave + local trailsd) |
| Phishable | Yes | Somewhat (token theft) | No (private key never transmitted) |
| Multi-device | Share password (bad) | SSO (good) | Separate keypair per device (best) |
| Revocation speed | Password change (slow) | Token expiry (minutes) | Instant (delete grant row) |
| Offline operation | No | Limited (cached tokens) | Yes (local trailsd has grant table) |
| Setup complexity | Create account | OAuth flow, consent screen | Generate keypair, scan QR |
| Grandmother-friendly | Maybe | No | Scan QR, done |

## The Physical Key Analogy

This entire model is a digital version of physical keys:

| Physical world | TRAILS keypair |
|---|---|
| Key | Ed25519 private key (in Secure Enclave) |
| Lock | Device's grant table (in trailsd Postgres) |
| Keyring | TRAILS app with multiple personas |
| Giving someone a key | Registering their public key with a grant |
| Taking back a key | Removing the grant |
| Lost keyring | Revoke all public keys for that keyring |
| Master key | Keypair with cascade grant on root of house tree |
| Room key | Keypair with grant on single room subtree |
| Timed key (hotel) | Keypair with expiresAt grant |
| Valet key (limited) | Keypair with reduced roles |

People already understand physical keys. TRAILS keypair identity maps 1:1. No new mental model needed.

## Spec Integration Notes

- §16 (Authorization and RBAC): Add keypair-external as third authorization path alongside tree-internal and OAuth-external.
- §22 (Postgres Schema): Add `keypair_grants` table.
- §23 (REST API): Add `POST/DELETE /api/v1/grants/keypair` endpoints.
- §25 (CLI Client): Add `trails grant add/remove/list` commands for keypair grants.
- Client-kotlin and client-swift: Must integrate with platform Secure Enclave (Android Keystore / iOS Secure Enclave) for hardware-protected private keys.
- Mobile app UX: QR code scanning, persona management, biometric gate for signing.

---

*Addendum 2 introduces keypair-based identity as the third authorization path — complementing tree-internal (process keypairs) and OAuth-external (enterprise). Uses the same Ed25519 primitive already in the protocol. No new crypto, no new infrastructure. The public key is the identity. The phone is a keyring. The grant table is the lock.*