from __future__ import annotations

import asyncio
from io import BytesIO
import json
from pathlib import Path
import subprocess
import struct
import sys
import threading
import time

import msgpack
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


def _wire_frame(**changes: object) -> bytes:
    raw = json.loads((FIXTURES / "session-start.json").read_text(encoding="utf-8"))
    raw.update(changes)
    payload = msgpack.packb(raw, use_bin_type=True)
    return struct.pack(">I", len(payload)) + payload


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


@pytest.mark.parametrize(
    ("changes", "message"),
    [({"version": 65_536}, "version"), ({"kind": "bogus"}, "kind")],
)
def test_run_rejects_nonrecoverable_wire_envelopes_before_dispatch(changes, message):
    handled: list[Envelope] = []

    async def handler(command: Envelope) -> None:
        handled.append(command)

    transport = SidecarTransport(BytesIO(_wire_frame(**changes)), BytesIO())

    with pytest.raises(FrameError, match=message):
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


class _CloseableBlockingReadStream:
    def __init__(self, data: bytes) -> None:
        self._data = data
        self._offset = 0
        self.started = threading.Event()
        self.finished = threading.Event()
        self.release = threading.Event()
        self.closed = False

    @property
    def unread_data(self) -> bytes:
        return self._data[self._offset :]

    def read(self, size: int) -> bytes:
        self.started.set()
        assert self.release.wait(timeout=1)
        try:
            if self.closed:
                return b""
            chunk = self._data[self._offset : self._offset + size]
            self._offset += len(chunk)
            return chunk
        finally:
            self.finished.set()

    def close(self) -> None:
        self.closed = True
        self.release.set()


def test_cancelling_run_closes_and_joins_the_active_read_worker():
    command = _envelope("session-start.json")
    input_stream = _CloseableBlockingReadStream(encode_frame(command))
    transport = SidecarTransport(input_stream, BytesIO())
    handled: list[Envelope] = []

    async def handler(received: Envelope) -> None:
        handled.append(received)

    async def verify() -> None:
        task = asyncio.create_task(transport.run(handler))
        await asyncio.to_thread(input_stream.started.wait, 1)
        task.cancel()
        try:
            with pytest.raises(asyncio.CancelledError):
                await asyncio.wait_for(task, timeout=0.5)
        finally:
            input_stream.release.set()

    asyncio.run(verify())

    assert input_stream.closed
    assert input_stream.finished.is_set()
    assert input_stream.unread_data == encode_frame(command)
    assert handled == []


class _CancellationBlockingWriteStream(BytesIO):
    def __init__(self) -> None:
        super().__init__()
        self._state_lock = threading.Lock()
        self.active_writes = 0
        self.max_active_writes = 0
        self.first_write_started = threading.Event()
        self.second_write_started = threading.Event()
        self.release_first_write = threading.Event()

    def write(self, data: bytes) -> int:
        with self._state_lock:
            self.active_writes += 1
            self.max_active_writes = max(self.max_active_writes, self.active_writes)
            is_first_write = self.active_writes == 1 and not self.first_write_started.is_set()
        try:
            if is_first_write:
                self.first_write_started.set()
                assert self.release_first_write.wait(timeout=1)
            else:
                self.second_write_started.set()
            return super().write(data)
        finally:
            with self._state_lock:
                self.active_writes -= 1


def test_cancelling_send_keeps_the_write_lock_until_the_worker_finishes():
    output = _CancellationBlockingWriteStream()
    transport = SidecarTransport(BytesIO(), output)
    first = _envelope("sidecar-ready.json")
    second = _envelope("sidecar-ready.json", sequence=2)

    async def verify() -> None:
        first_send = asyncio.create_task(transport.send(first))
        await asyncio.to_thread(output.first_write_started.wait, 1)
        first_send.cancel()
        second_send = asyncio.create_task(transport.send(second))
        await asyncio.sleep(0.05)
        assert not output.second_write_started.is_set()
        output.release_first_write.set()
        with pytest.raises(asyncio.CancelledError):
            await first_send
        await second_send

    asyncio.run(verify())

    output.seek(0)
    assert output.max_active_writes == 1
    assert read_frame(output) == first
    assert read_frame(output) == second
    assert read_frame(output) is None


def test_repeated_cancellation_keeps_the_write_lock_until_the_worker_finishes():
    output = _CancellationBlockingWriteStream()
    transport = SidecarTransport(BytesIO(), output)
    first = _envelope("sidecar-ready.json")
    second = _envelope("sidecar-ready.json", sequence=2)

    async def verify() -> None:
        first_send = asyncio.create_task(transport.send(first))
        await asyncio.to_thread(output.first_write_started.wait, 1)
        first_send.cancel()
        await asyncio.sleep(0)
        first_send.cancel()
        second_send = asyncio.create_task(transport.send(second))
        try:
            await asyncio.sleep(0.05)
            assert not output.second_write_started.is_set()
            assert output.max_active_writes == 1
        finally:
            output.release_first_write.set()
        with pytest.raises(asyncio.CancelledError):
            await first_send
        await second_send

    asyncio.run(verify())

    output.seek(0)
    assert output.max_active_writes == 1
    assert read_frame(output) == first
    assert read_frame(output) == second
    assert read_frame(output) is None


class _RepeatedCancellationReadStream:
    def __init__(self, data: bytes) -> None:
        self._data = data
        self._offset = 0
        self.started = threading.Event()
        self.close_called = threading.Event()
        self.release = threading.Event()
        self.finished = threading.Event()
        self.closed = False

    @property
    def unread_data(self) -> bytes:
        return self._data[self._offset :]

    def read(self, size: int) -> bytes:
        self.started.set()
        assert self.release.wait(timeout=1)
        try:
            if self.closed:
                return b""
            chunk = self._data[self._offset : self._offset + size]
            self._offset += len(chunk)
            return chunk
        finally:
            self.finished.set()

    def close(self) -> None:
        self.closed = True
        self.close_called.set()


def test_repeated_cancellation_keeps_read_cleanup_active_until_the_worker_finishes():
    command = _envelope("session-start.json")
    input_stream = _RepeatedCancellationReadStream(encode_frame(command))
    transport = SidecarTransport(input_stream, BytesIO())
    handled: list[Envelope] = []

    async def handler(received: Envelope) -> None:
        handled.append(received)

    async def verify() -> None:
        run_task = asyncio.create_task(transport.run(handler))
        await asyncio.to_thread(input_stream.started.wait, 1)
        run_task.cancel()
        await asyncio.to_thread(input_stream.close_called.wait, 1)
        run_task.cancel()
        try:
            await asyncio.sleep(0.05)
            assert not run_task.done()
            assert not input_stream.finished.is_set()
        finally:
            input_stream.release.set()
        with pytest.raises(asyncio.CancelledError):
            await run_task

    asyncio.run(verify())

    assert input_stream.closed
    assert input_stream.finished.is_set()
    assert input_stream.unread_data == encode_frame(command)
    assert handled == []


class _FailingAfterCancellationWriteStream:
    def __init__(self) -> None:
        self.started = threading.Event()
        self.release = threading.Event()

    def write(self, data: bytes) -> int:
        self.started.set()
        assert self.release.wait(timeout=1)
        return 0


def test_cancelling_send_reraises_cancellation_after_the_worker_fails():
    output = _FailingAfterCancellationWriteStream()
    transport = SidecarTransport(BytesIO(), output)

    async def verify() -> None:
        send_task = asyncio.create_task(transport.send(_envelope("sidecar-ready.json")))
        await asyncio.to_thread(output.started.wait, 1)
        send_task.cancel()
        output.release.set()
        with pytest.raises(asyncio.CancelledError):
            await send_task

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
