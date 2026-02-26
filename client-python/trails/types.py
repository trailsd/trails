"""TRAILS type definitions."""

from __future__ import annotations

import json
import base64
import uuid
from dataclasses import dataclass, field, asdict
from typing import Optional


@dataclass
class Originator:
    sub: Optional[str] = None
    groups: Optional[list[str]] = None


@dataclass
class TrailsConfig:
    """Decoded TRAILS_INFO envelope (spec §5)."""

    v: int = 1
    app_id: str = ""
    parent_id: Optional[str] = None
    app_name: str = ""
    server_ep: str = ""
    server_pub_key: Optional[str] = None
    sec_level: str = "open"
    scheduled_at: Optional[int] = None
    start_deadline: Optional[int] = 300
    originator: Optional[Originator] = None
    role_refs: list[str] = field(default_factory=list)
    tags: Optional[dict] = None

    def to_json(self) -> str:
        """Serialize to camelCase JSON matching the wire format."""
        d = {
            "v": self.v,
            "appId": self.app_id,
            "parentId": self.parent_id,
            "appName": self.app_name,
            "serverEp": self.server_ep,
            "serverPubKey": self.server_pub_key,
            "secLevel": self.sec_level,
            "scheduledAt": self.scheduled_at,
            "startDeadline": self.start_deadline,
            "roleRefs": self.role_refs,
            "tags": self.tags,
        }
        if self.originator:
            d["originator"] = {
                "sub": self.originator.sub,
                "groups": self.originator.groups,
            }
        return json.dumps(d)

    @classmethod
    def from_json(cls, s: str) -> TrailsConfig:
        """Deserialize from camelCase JSON."""
        d = json.loads(s)
        originator = None
        if "originator" in d and d["originator"]:
            originator = Originator(
                sub=d["originator"].get("sub"),
                groups=d["originator"].get("groups"),
            )
        return cls(
            v=d.get("v", 1),
            app_id=d.get("appId", ""),
            parent_id=d.get("parentId"),
            app_name=d.get("appName", ""),
            server_ep=d.get("serverEp", ""),
            server_pub_key=d.get("serverPubKey"),
            sec_level=d.get("secLevel", "open"),
            scheduled_at=d.get("scheduledAt"),
            start_deadline=d.get("startDeadline", 300),
            originator=originator,
            role_refs=d.get("roleRefs", []),
            tags=d.get("tags"),
        )

    def encode(self) -> str:
        """Encode as base64 string for TRAILS_INFO env var."""
        return base64.b64encode(self.to_json().encode()).decode()

    @classmethod
    def decode(cls, b64: str) -> TrailsConfig:
        """Decode from base64 TRAILS_INFO env var."""
        raw = base64.b64decode(b64.strip())
        return cls.from_json(raw.decode())
