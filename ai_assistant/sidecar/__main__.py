"""Command-line entry point for the Python sidecar."""

from __future__ import annotations

import argparse
import asyncio
import json
import logging
import sys

from .protocol import Envelope, PROTOCOL_VERSION
from .transport import SidecarTransport


def _configure_logging() -> None:
    logging.basicConfig(level=logging.INFO, stream=sys.stderr)


async def _discard_command(_: Envelope) -> None:
    """Provide a binary-safe default loop until command handlers are wired."""


async def _run_transport() -> None:
    transport = SidecarTransport(sys.stdin.buffer, sys.stdout.buffer)
    await transport.run(_discard_command)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--healthcheck", action="store_true")
    arguments = parser.parse_args(argv)

    if arguments.healthcheck:
        print(json.dumps({"status": "ok", "protocol_version": PROTOCOL_VERSION}, separators=(",", ":")))
        return 0

    _configure_logging()
    asyncio.run(_run_transport())
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
