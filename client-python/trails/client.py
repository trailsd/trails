"""
TRAILS Python client.

Two lines to integrate:

    g = TrailsClient.init()
    g.status({"phase": "processing", "progress": 0.5})

If TRAILS_INFO is absent, init() returns a no-op client where all
methods silently succeed. Zero overhead.

See TRAILS-SPEC.md §24 for the full API surface.
"""

from __future__ import annotations

import asyncio
import json
import logging
import os
import platform
import queue
import random
import socket
import sys
import threading
import time
import uuid as uuid_mod
from typing import Any, Callable, Optional

from .types import Originator, TrailsConfig

try:
    import nacl.signing  # type: ignore
    import nacl.encoding  # type: ignore

    _HAS_NACL = True
except ImportError:
    _HAS_NACL = False

try:
    import websockets  # type: ignore
    import websockets.exceptions  # type: ignore

    _HAS_WS = True
except ImportError:
    _HAS_WS = False

logger = logging.getLogger("trails")


class TrailsClient:
    """TRAILS client — send status, results, and errors to the TRAILS server.

    Internally runs a background thread with an asyncio event loop that
    manages the WebSocket connection (including reconnection with exponential
    backoff + jitter). Public methods are synchronous and never block on I/O.

    If TRAILS_INFO was absent, this is a no-op client.
    """

    def __init__(self, config: Optional[TrailsConfig], noop: bool = False):
        self._config = config
        self._noop = noop
        self._seq = 0
        self._connected = threading.Event()
        self._queue: queue.Queue = queue.Queue(maxsize=1024)
        self._shutdown_event = threading.Event()
        self._thread: Optional[threading.Thread] = None

        if not noop and config:
            # Generate Ed25519 keypair.
            if _HAS_NACL:
                self._signing_key = nacl.signing.SigningKey.generate()
                pub_bytes = self._signing_key.verify_key.encode()
                import base64 as b64

                self._pub_key_str = (
                    "ed25519:" + b64.b64encode(pub_bytes).decode()
                )
            else:
                self._signing_key = None
                self._pub_key_str = "ed25519:no-nacl-installed"

            # Start background thread.
            self._thread = threading.Thread(
                target=self._run_background, daemon=True, name="trails-ws"
            )
            self._thread.start()

    # ── Factory methods ──────────────────────────────────────

    @classmethod
    def init(cls) -> TrailsClient:
        """Read TRAILS_INFO from environment. Returns no-op if absent."""
        b64 = os.environ.get("TRAILS_INFO")
        if not b64:
            logger.debug("TRAILS_INFO not set, using no-op client")
            return cls(config=None, noop=True)

        try:
            config = TrailsConfig.decode(b64)
        except Exception as e:
            logger.warning("TRAILS_INFO decode failed: %s, using no-op", e)
            return cls(config=None, noop=True)

        return cls(config=config, noop=False)

    @classmethod
    def init_with(cls, config: TrailsConfig) -> TrailsClient:
        """Initialize with explicit config (non-env-var delivery, spec §5)."""
        return cls(config=config, noop=False)

    # ── Public API ───────────────────────────────────────────

    @property
    def is_active(self) -> bool:
        """True if this is a real client (not no-op)."""
        return not self._noop

    @property
    def is_connected(self) -> bool:
        """True if WebSocket is currently connected."""
        return self._connected.is_set()

    def status(self, payload: dict) -> None:
        """Send a status update (spec §9)."""
        self._send("Status", payload)

    def result(self, payload: dict) -> None:
        """Send a business result (spec §9). Transitions app to 'done'."""
        self._send("Result", payload)

    def error(self, msg: str, detail: Optional[dict] = None) -> None:
        """Send a structured error (spec §9). Transitions app to 'error'."""
        self._send("Error", {"message": msg, "detail": detail})

    def on(self, action: str) -> Callable:
        """Decorator for control command handlers (Phase 3)."""

        def decorator(func: Callable) -> Callable:
            # Phase 3: register handler.
            return func

        return decorator

    def on_cancel(self, grace_seconds: int = 30, hook: Optional[Callable] = None) -> None:
        """Register cancel hook (Phase 3)."""
        pass

    def create_child(self, name: str) -> TrailsConfig:
        """Generate TRAILS_INFO for a child process."""
        if not self._config:
            raise RuntimeError("cannot create child from no-op client")

        child_id = str(uuid_mod.uuid4())
        return TrailsConfig(
            v=1,
            app_id=child_id,
            parent_id=self._config.app_id,
            app_name=name,
            server_ep=self._config.server_ep,
            server_pub_key=self._config.server_pub_key,
            sec_level=self._config.sec_level,
            scheduled_at=int(time.time() * 1000),
            start_deadline=self._config.start_deadline,
            originator=self._config.originator,
            role_refs=list(self._config.role_refs),
            tags=None,
        )

    def shutdown(self) -> None:
        """Graceful shutdown. Sends disconnect, stops background thread."""
        if self._noop or not self._thread:
            return

        try:
            self._queue.put_nowait(
                ("disconnect", {"reason": "completed"})
            )
        except queue.Full:
            pass

        self._shutdown_event.set()
        self._thread.join(timeout=5.0)

    # ── Internal ─────────────────────────────────────────────

    def _send(self, msg_type: str, payload: dict) -> None:
        """Enqueue a message. No-op if client is inactive or queue is full."""
        if self._noop:
            return
        self._seq += 1
        try:
            self._queue.put_nowait((msg_type, payload, self._seq))
        except queue.Full:
            logger.debug("message dropped (queue full)")

    def _run_background(self) -> None:
        """Background thread entry: runs asyncio event loop."""
        loop = asyncio.new_event_loop()
        asyncio.set_event_loop(loop)
        try:
            loop.run_until_complete(self._ws_loop())
        except Exception as e:
            logger.error("background task crashed: %s", e)
        finally:
            loop.close()

    async def _ws_loop(self) -> None:
        """WebSocket connection loop with reconnection."""
        if not _HAS_WS:
            logger.error("websockets package not installed, TRAILS disabled")
            return

        ws_url = self._normalize_ws_url(self._config.server_ep)
        attempt = 0
        first_connect = True
        last_seq = 0

        while not self._shutdown_event.is_set():
            try:
                async with websockets.connect(ws_url) as ws:
                    logger.info("WebSocket connected to %s", ws_url)
                    attempt = 0

                    # ── Register ─────────────────────────────
                    if first_connect:
                        reg = self._make_register_msg()
                    else:
                        reg = self._make_re_register_msg(last_seq)

                    await ws.send(json.dumps(reg))

                    # Wait for Registered ack.
                    try:
                        resp = await asyncio.wait_for(ws.recv(), timeout=10.0)
                        logger.debug("server: %s", resp)
                        resp_data = json.loads(resp)
                        if resp_data.get("type") == "error":
                            logger.error("registration rejected: %s", resp)
                            raise Exception("rejected")
                    except asyncio.TimeoutError:
                        logger.warning("registration timeout")
                        raise Exception("timeout")

                    self._connected.set()
                    first_connect = False

                    # ── Message loop ─────────────────────────
                    await self._message_loop(ws)

            except Exception as e:
                logger.debug("connection error: %s", e)

            self._connected.clear()

            if self._shutdown_event.is_set():
                break

            # Backoff.
            delay = self._backoff_delay(attempt)
            logger.debug("reconnecting in %.1fs (attempt %d)", delay, attempt)
            await asyncio.sleep(delay)
            attempt += 1

    async def _message_loop(self, ws) -> None:
        """Send queued messages, receive acks."""
        import websockets.exceptions

        while not self._shutdown_event.is_set():
            # Drain the queue (non-blocking).
            try:
                item = self._queue.get_nowait()
            except queue.Empty:
                # No messages — wait briefly then check for server messages.
                try:
                    msg = await asyncio.wait_for(ws.recv(), timeout=0.1)
                    logger.debug("server: %s", msg)
                except asyncio.TimeoutError:
                    pass
                except websockets.exceptions.ConnectionClosed:
                    break
                continue

            if item[0] == "disconnect":
                disc = {
                    "type": "disconnect",
                    "app_id": self._config.app_id,
                    "reason": item[1].get("reason", "completed"),
                }
                try:
                    await ws.send(json.dumps(disc))
                except Exception:
                    pass
                return  # exit loop

            msg_type, payload, seq = item
            wire = {
                "type": "message",
                "app_id": self._config.app_id,
                "header": {
                    "msg_type": msg_type,
                    "timestamp": int(time.time() * 1000),
                    "seq": seq,
                    "correlation_id": None,
                },
                "payload": payload,
                "sig": None,
            }

            try:
                await ws.send(json.dumps(wire))
            except websockets.exceptions.ConnectionClosed:
                # Re-enqueue if possible.
                try:
                    self._queue.put_nowait(item)
                except queue.Full:
                    pass
                break

            # Try to read ack.
            try:
                ack = await asyncio.wait_for(ws.recv(), timeout=1.0)
                logger.debug("ack: %s", ack)
            except asyncio.TimeoutError:
                pass
            except websockets.exceptions.ConnectionClosed:
                break

    def _make_register_msg(self) -> dict:
        return {
            "type": "register",
            "app_id": self._config.app_id,
            "parent_id": self._config.parent_id,
            "app_name": self._config.app_name,
            "child_pub_key": self._pub_key_str,
            "process_info": self._collect_process_info(),
            "role_refs": self._config.role_refs,
            "sig": None,
        }

    def _make_re_register_msg(self, last_seq: int) -> dict:
        return {
            "type": "re_register",
            "app_id": self._config.app_id,
            "last_seq": last_seq,
            "pub_key": self._pub_key_str,
            "sig": None,
        }

    @staticmethod
    def _collect_process_info() -> dict:
        """Collect process identity (spec §6)."""
        ns = None
        try:
            with open(
                "/var/run/secrets/kubernetes.io/serviceaccount/namespace"
            ) as f:
                ns = f.read().strip()
        except FileNotFoundError:
            pass

        return {
            "pid": os.getpid(),
            "ppid": os.getppid(),
            "uid": os.getuid() if hasattr(os, "getuid") else 0,
            "gid": os.getgid() if hasattr(os, "getgid") else 0,
            "hostname": socket.gethostname(),
            "node_name": os.environ.get("NODE_NAME"),
            "pod_ip": os.environ.get("POD_IP"),
            "namespace": ns or os.environ.get("POD_NAMESPACE"),
            "start_time": int(time.time() * 1000),
            "executable": sys.executable,
        }

    @staticmethod
    def _normalize_ws_url(ep: str) -> str:
        url = ep.replace("https://", "wss://").replace("http://", "ws://")
        if "/ws" not in url:
            url = url.rstrip("/") + "/ws"
        return url

    @staticmethod
    def _backoff_delay(attempt: int) -> float:
        """Exponential backoff with jitter (spec §19)."""
        base = min(0.1 * (2**attempt), 30.0)
        jitter = random.random() * base * 0.5
        return base + jitter
