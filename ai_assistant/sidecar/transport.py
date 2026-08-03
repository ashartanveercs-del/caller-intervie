"""Async transport loop for framed sidecar commands."""

from __future__ import annotations

import asyncio
from collections.abc import Awaitable, Callable
from typing import BinaryIO

from .framing import read_frame, write_frame
from .protocol import Envelope


CommandHandler = Callable[[Envelope], Awaitable[Envelope | None]]


class SidecarTransport:
    """Dispatch framed commands while keeping blocking stream I/O off the event loop."""

    def __init__(self, input_stream: BinaryIO, output_stream: BinaryIO) -> None:
        self._input_stream = input_stream
        self._output_stream = output_stream
        self._write_lock = asyncio.Lock()
        self._active_read: asyncio.Task[Envelope | None] | None = None
        self._input_closed = False

    async def run(self, handler: CommandHandler) -> None:
        """Read and dispatch commands until the input stream reaches clean EOF."""
        while (command := await self._read_command()) is not None:
            response = await handler(command)
            if response is not None:
                await self.send(response)

    async def send(self, envelope: Envelope) -> None:
        """Write one event atomically with respect to all other transport writes."""
        async with self._write_lock:
            await self._wait_for_worker(
                asyncio.create_task(asyncio.to_thread(write_frame, self._output_stream, envelope))
            )

    async def close(self) -> None:
        """Close input and wait for any in-flight read to finish."""
        if not self._input_closed:
            self._input_closed = True
            await asyncio.to_thread(self._input_stream.close)

        active_read = self._active_read
        if active_read is not None:
            await asyncio.shield(active_read)

    async def _read_command(self) -> Envelope | None:
        read_task = asyncio.create_task(asyncio.to_thread(read_frame, self._input_stream))
        self._active_read = read_task
        try:
            return await asyncio.shield(read_task)
        except asyncio.CancelledError:
            try:
                await self.close()
            except (asyncio.CancelledError, Exception):
                pass
            raise
        finally:
            if self._active_read is read_task and read_task.done():
                self._active_read = None

    async def _wait_for_worker(self, worker: asyncio.Task[None]) -> None:
        try:
            await asyncio.shield(worker)
        except asyncio.CancelledError:
            try:
                await asyncio.shield(worker)
            except (asyncio.CancelledError, Exception):
                pass
            raise
