"""Tests for the TRAILS Python client."""

import os
import json
import base64

import pytest

from trails import TrailsClient, TrailsConfig, Originator


class TestTrailsConfig:
    def test_encode_decode_roundtrip(self):
        config = TrailsConfig(
            app_id="550e8400-e29b-41d4-a716-446655440000",
            parent_id="440e7300-d18a-30c3-b605-335544330000",
            app_name="test-task",
            server_ep="ws://localhost:8443/ws",
            sec_level="open",
            scheduled_at=1740000000000,
            start_deadline=300,
            role_refs=["sre-on-call"],
        )
        encoded = config.encode()
        decoded = TrailsConfig.decode(encoded)

        assert decoded.app_id == config.app_id
        assert decoded.parent_id == config.parent_id
        assert decoded.app_name == config.app_name
        assert decoded.server_ep == config.server_ep
        assert decoded.role_refs == ["sre-on-call"]

    def test_from_json_camel_case(self):
        j = json.dumps(
            {
                "v": 1,
                "appId": "aaa",
                "parentId": "bbb",
                "appName": "test",
                "serverEp": "ws://host:8443/ws",
                "secLevel": "open",
                "roleRefs": ["reader"],
            }
        )
        config = TrailsConfig.from_json(j)
        assert config.app_id == "aaa"
        assert config.parent_id == "bbb"
        assert config.role_refs == ["reader"]

    def test_originator_roundtrip(self):
        config = TrailsConfig(
            app_id="aaa",
            app_name="test",
            server_ep="ws://host/ws",
            originator=Originator(sub="alice@co.com", groups=["eng"]),
        )
        encoded = config.encode()
        decoded = TrailsConfig.decode(encoded)
        assert decoded.originator is not None
        assert decoded.originator.sub == "alice@co.com"
        assert decoded.originator.groups == ["eng"]


class TestNoopClient:
    def test_noop_when_no_env(self):
        os.environ.pop("TRAILS_INFO", None)
        g = TrailsClient.init()
        assert not g.is_active
        assert not g.is_connected

    def test_noop_methods_succeed(self):
        os.environ.pop("TRAILS_INFO", None)
        g = TrailsClient.init()
        # All methods should succeed silently.
        g.status({"progress": 0.5})
        g.result({"done": True})
        g.error("test error")
        g.shutdown()

    def test_noop_on_invalid_base64(self):
        os.environ["TRAILS_INFO"] = "not-valid-base64!!!"
        g = TrailsClient.init()
        assert not g.is_active
        os.environ.pop("TRAILS_INFO", None)


class TestCreateChild:
    def test_create_child_config(self):
        parent_config = TrailsConfig(
            app_id="parent-uuid",
            app_name="parent",
            server_ep="ws://trails:8443/ws",
            sec_level="open",
            role_refs=["monitoring"],
        )
        g = TrailsClient.init_with(parent_config)
        child_config = g.create_child("step-1")

        assert child_config.parent_id == "parent-uuid"
        assert child_config.app_name == "step-1"
        assert child_config.server_ep == "ws://trails:8443/ws"
        assert child_config.role_refs == ["monitoring"]
        assert child_config.app_id != parent_config.app_id
        g.shutdown()

    def test_create_child_from_noop_raises(self):
        os.environ.pop("TRAILS_INFO", None)
        g = TrailsClient.init()
        with pytest.raises(RuntimeError):
            g.create_child("step-1")


class TestNormalizeUrl:
    def test_passthrough_ws(self):
        assert TrailsClient._normalize_ws_url("ws://host:8443/ws") == "ws://host:8443/ws"

    def test_http_to_ws(self):
        assert TrailsClient._normalize_ws_url("http://host:8443") == "ws://host:8443/ws"

    def test_https_to_wss(self):
        assert TrailsClient._normalize_ws_url("https://host:8443/ws") == "wss://host:8443/ws"

    def test_adds_ws_path(self):
        assert TrailsClient._normalize_ws_url("ws://host:8443") == "ws://host:8443/ws"
