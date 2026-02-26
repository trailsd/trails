"""
TRAILS — Tree-scoped Relay for Application Info, Lifecycle, and Signaling.

Usage:
    from trails import TrailsClient

    g = TrailsClient.init()
    g.status({"phase": "processing", "progress": 0.5})
    g.result({"rows": 100000, "pii_cols": 4})
    g.shutdown()
"""

from .client import TrailsClient
from .types import Originator, TrailsConfig

__version__ = "0.1.0"
__all__ = ["TrailsClient", "TrailsConfig", "Originator"]
