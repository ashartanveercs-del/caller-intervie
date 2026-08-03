"""Command-line entry point for the Python sidecar."""

from __future__ import annotations

import argparse
import asyncio
import json
import logging
import sys

from ai_assistant.config import Config
from ai_assistant.runtime import build_runtime

from ai_assistant.sidecar.adapter import RuntimeProtocolAdapter
from ai_assistant.sidecar.protocol import PROTOCOL_VERSION
from ai_assistant.sidecar.transport import SidecarTransport


def _configure_logging() -> None:
    logging.basicConfig(level=logging.INFO, stream=sys.stderr)


async def _run_transport() -> None:
    transport = SidecarTransport(sys.stdin.buffer, sys.stdout.buffer)
    runtime = build_runtime(Config.from_env())
    adapter = RuntimeProtocolAdapter(runtime, transport.send)
    await adapter.emit_ready()
    try:
        await transport.run(adapter.handle)
    finally:
        await runtime.stop_session()


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--healthcheck", action="store_true")
    parser.add_argument("--self-test", action="store_true")
    arguments = parser.parse_args(argv)

    if arguments.healthcheck:
        print(json.dumps({"status": "ok", "protocol_version": PROTOCOL_VERSION}, separators=(",", ":")))
        return 0
    if arguments.self_test:
        from ai_assistant.sidecar.self_test import run

        print(json.dumps(run(), separators=(",", ":")))
        return 0

    _configure_logging()
    asyncio.run(_run_transport())
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
