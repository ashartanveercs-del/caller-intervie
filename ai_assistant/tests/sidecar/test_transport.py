from __future__ import annotations

import asyncio
from io import BytesIO
import json
from pathlib import Path
import subprocess
import sys
import threading
import time

import pytest

from ai_assistant.sidecar.framing import FrameError, encode_frame, read_frame
from ai_assistant.sidecar.protocol import Envelope
from ai_assistant.sidecar.transport import SidecarTransport


FIXTURES = Path(__file__).resolve().parents[3] / "protocol" / "v1" / "fixtures"
PROJECT_ROOT = Path(__file__).resolve().parents[3]


def _envelope(name: str, *, sequence: int | None = None) -> Envelope:
    raw = json.loads((FIXTURES / name).read_text(encoding="utf-8"))
    if sequence is not None:
        raw["sequence"] = sequence
    return Envelope.from_dict(raw)


def test_run_dispatches_commands_and_writes_handler_responses():
    first = _envelope("session-start.json")
    second = _envelope("session-start.json", sequence=2)
    response = _envelope("sidecar-ready.json")
    handled: list[Envelope] = []

    async def handler(command: Envelope) -> Envelope:
        handled.append(command)
        return response

    output = BytesIO()
    transport = SidecarTransport(BytesIO(encode_frame(first) + encode_frame(second)), output)

    asyncio.run(transport.run(handler))

    output.seek(0)
    assert handled == [first, second]
    assert read_frame(output) == response
    assert read_frame(output) == response
    assert read_frame(output) is None


def test_run_rejects_invalid_envelopes_before_dispatch():
    invalid_payload = b"\x81\xa7version\x01"
    handled: list[Envelope] = []

    async def handler(command: Envelope) -> None:
        handled.append(command)

    transport = SidecarTransport(
        BytesIO(len(invalid_payload).to_bytes(4, "big") + invalid_payload), BytesIO()
    )

    with pytest.raises(FrameError, match="envelope"):
        asyncio.run(transport.run(handler))

    assert handled == []


class _ConcurrentWriteDetectingStream(BytesIO):
    def __init__(self) -> None:
        super().__init__()
        self._state_lock = threading.Lock()
        self.active_writes = 0
        self.max_active_writes = 0

    def write(self, data: bytes) -> int:
        with self._state_lock:
            self.active_writes += 1
            self.max_active_writes = max(self.max_active_writes, self.active_writes)
        try:
            time.sleep(0.02)
            return super().write(data)
        finally:
            with self._state_lock:
                self.active_writes -= 1


def test_send_serializes_concurrent_writes():
    output = _ConcurrentWriteDetectingStream()
    first = _envelope("sidecar-ready.json")
    second = _envelope("sidecar-ready.json", sequence=2)
    transport = SidecarTransport(BytesIO(), output)

    async def send_both() -> None:
        await asyncio.gather(transport.send(first), transport.send(second))

    asyncio.run(send_both())

    output.seek(0)
    assert output.max_active_writes == 1
    assert {read_frame(output).sequence, read_frame(output).sequence} == {0, 2}
    assert read_frame(output) is None


class _BlockingReadStream:
    def __init__(self) -> None:
        self.started = threading.Event()
        self.release = threading.Event()

    def read(self, size: int) -> bytes:
        self.started.set()
        assert self.release.wait(timeout=1)
        return b""


def test_run_reads_on_a_worker_thread_without_blocking_the_event_loop():
    input_stream = _BlockingReadStream()
    transport = SidecarTransport(input_stream, BytesIO())

    async def verify() -> None:
        async def handler(_: Envelope) -> None:
            return None

        task = asyncio.create_task(transport.run(handler))
        await asyncio.to_thread(input_stream.started.wait, 1)
        loop_is_running = asyncio.Event()
        asyncio.get_running_loop().call_soon(loop_is_running.set)
        await asyncio.wait_for(loop_is_running.wait(), timeout=0.1)
        input_stream.release.set()
        await task

    asyncio.run(verify())


def test_healthcheck_writes_only_one_json_object_to_stdout():
    completed = subprocess.run(
        [sys.executable, "-m", "ai_assistant.sidecar", "--healthcheck"],
        cwd=PROJECT_ROOT,
        check=True,
        capture_output=True,
    )

    assert completed.stdout.decode("utf-8").strip() == '{"status":"ok","protocol_version":1}'
    assert completed.stderr == b""


def test_command_line_diagnostics_use_stderr_not_stdout():
    completed = subprocess.run(
        [sys.executable, "-m", "ai_assistant.sidecar", "--unknown-option"],
        cwd=PROJECT_ROOT,
        capture_output=True,
    )

    assert completed.returncode == 2
    assert completed.stdout == b""
    assert b"unrecognized arguments" in completed.stderr
