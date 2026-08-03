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

    async def run(self, handler: CommandHandler) -> None:
        """Read and dispatch commands until the input stream reaches clean EOF."""
        while (command := await asyncio.to_thread(read_frame, self._input_stream)) is not None:
            response = await handler(command)
            if response is not None:
                await self.send(response)

    async def send(self, envelope: Envelope) -> None:
        """Write one event atomically with respect to all other transport writes."""
        async with self._write_lock:
            await asyncio.to_thread(write_frame, self._output_stream, envelope)
