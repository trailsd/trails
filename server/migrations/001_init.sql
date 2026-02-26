-- ═══════════════════════════════════════════════════════════════
-- TRAILS Phase 1 — Initial Schema
-- All tables from spec §22 present. Phase 1 uses: apps, messages,
-- snapshots, crashes. Remaining tables (control_queue, role_refs,
-- grants, audit_log, active_sessions) are schema-ready for later phases.
-- ═══════════════════════════════════════════════════════════════

-- ───────────────────────────────────────────────────────────────
-- Core: apps
-- ───────────────────────────────────────────────────────────────

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
    pub_key             TEXT,

    -- Lifecycle
    status              TEXT NOT NULL DEFAULT 'scheduled'
        CHECK (status IN (
            'scheduled', 'connected', 'running',
            'done', 'error', 'crashed', 'cancelled',
            'start_failed', 'reconnecting', 'lost_contact'
        )),
    start_time          TIMESTAMPTZ,
    connected_at        TIMESTAMPTZ,
    disconnected_at     TIMESTAMPTZ,
    server_instance     TEXT,

    -- Configuration
    role_refs           TEXT[],
    metadata_json       JSONB,
    start_deadline      INTEGER DEFAULT 300,

    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_apps_status ON apps(status);
CREATE INDEX idx_apps_parent ON apps(parent_id);
CREATE INDEX idx_apps_namespace ON apps(namespace);
CREATE INDEX idx_apps_name ON apps(app_name);
CREATE INDEX idx_apps_originator ON apps(originator_sub);

-- ───────────────────────────────────────────────────────────────
-- Messages (data path)
-- ───────────────────────────────────────────────────────────────

CREATE TABLE messages (
    id                  BIGSERIAL PRIMARY KEY,
    app_id              UUID NOT NULL REFERENCES apps(app_id),
    direction           TEXT NOT NULL
        CHECK (direction IN ('in', 'out')),
    msg_type            TEXT NOT NULL
        CHECK (msg_type IN ('Status', 'Result', 'Error', 'Control')),
    seq                 BIGINT NOT NULL,
    correlation_id      TEXT,
    payload_json        JSONB,
    payload_bytes       BYTEA,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_messages_app ON messages(app_id, created_at);
CREATE INDEX idx_messages_type ON messages(app_id, msg_type);

-- ───────────────────────────────────────────────────────────────
-- Snapshots (state reporting)
-- ───────────────────────────────────────────────────────────────

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

-- ───────────────────────────────────────────────────────────────
-- Crashes
-- ───────────────────────────────────────────────────────────────

CREATE TABLE crashes (
    id                  BIGSERIAL PRIMARY KEY,
    app_id              UUID NOT NULL REFERENCES apps(app_id),
    detected_at         TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    crash_type          TEXT NOT NULL
        CHECK (crash_type IN (
            'connection_drop', 'heartbeat_timeout', 'never_started'
        )),
    gap_seconds         REAL,
    metadata_json       JSONB
);

CREATE INDEX idx_crashes_time ON crashes(detected_at DESC);
CREATE INDEX idx_crashes_app ON crashes(app_id);

-- ───────────────────────────────────────────────────────────────
-- Control queue (Phase 3, schema-ready)
-- ───────────────────────────────────────────────────────────────

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

-- ───────────────────────────────────────────────────────────────
-- RBAC: Role references (Phase 5, schema-ready)
-- ───────────────────────────────────────────────────────────────

CREATE TABLE role_refs (
    name                TEXT PRIMARY KEY,
    description         TEXT,
    grants              JSONB NOT NULL,
    namespace_scope     TEXT,
    created_by          TEXT NOT NULL,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE role_refs_history (
    id                  BIGSERIAL PRIMARY KEY,
    role_ref_name       TEXT NOT NULL,
    action              TEXT NOT NULL
        CHECK (action IN ('created', 'updated', 'deleted')),
    old_grants          JSONB,
    new_grants          JSONB,
    changed_by          TEXT NOT NULL,
    changed_at          TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- ───────────────────────────────────────────────────────────────
-- RBAC: Per-app grants (Phase 5, schema-ready)
-- ───────────────────────────────────────────────────────────────

CREATE TABLE grants (
    id                  BIGSERIAL PRIMARY KEY,
    app_id              UUID NOT NULL REFERENCES apps(app_id),
    subject             TEXT NOT NULL,
    roles               TEXT[] NOT NULL,
    granted_by          UUID NOT NULL,
    expires_at          TIMESTAMPTZ,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_grants_app ON grants(app_id);
CREATE INDEX idx_grants_subject ON grants(subject);
CREATE INDEX idx_grants_expiry ON grants(expires_at) WHERE expires_at IS NOT NULL;

-- ───────────────────────────────────────────────────────────────
-- Audit log
-- ───────────────────────────────────────────────────────────────

CREATE TABLE audit_log (
    id                  BIGSERIAL PRIMARY KEY,
    timestamp           TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    action              TEXT NOT NULL,
    target_app_id       UUID,
    cascade             BOOLEAN DEFAULT false,
    payload_json        JSONB,
    auth_domain         TEXT NOT NULL
        CHECK (auth_domain IN ('tree', 'external')),
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

-- ───────────────────────────────────────────────────────────────
-- Session management (Phase 5, schema-ready)
-- ───────────────────────────────────────────────────────────────

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

-- ───────────────────────────────────────────────────────────────
-- Helper: auto-update updated_at on apps
-- ───────────────────────────────────────────────────────────────

CREATE OR REPLACE FUNCTION update_updated_at()
RETURNS TRIGGER AS $$
BEGIN
    NEW.updated_at = NOW();
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER apps_updated_at
    BEFORE UPDATE ON apps
    FOR EACH ROW
    EXECUTE FUNCTION update_updated_at();
