"""Async transport loop for framed sidecar commands."""

from __future__ import annotations

import asyncio
from collections.abc import Awaitable, Callable
from typing import Any, BinaryIO

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
            await self._drain_workers(
                [asyncio.create_task(asyncio.to_thread(write_frame, self._output_stream, envelope))]
            )

    async def close(self) -> None:
        """Close input and wait for any in-flight read to finish."""
        workers: list[asyncio.Task[Any]] = []
        close_task = self._start_close_input()
        if close_task is not None:
            workers.append(close_task)

        active_read = self._active_read
        if active_read is not None:
            workers.append(active_read)

        if workers:
            await self._drain_workers(workers)

    async def _read_command(self) -> Envelope | None:
        read_task = asyncio.create_task(asyncio.to_thread(read_frame, self._input_stream))
        self._active_read = read_task
        try:
            return (await self._drain_workers([read_task], on_cancel=self._start_close_input))[0]
        finally:
            if self._active_read is read_task and read_task.done():
                self._active_read = None

    def _start_close_input(self) -> asyncio.Task[None] | None:
        if self._input_closed:
            return None
        self._input_closed = True
        return asyncio.create_task(asyncio.to_thread(self._input_stream.close))

    async def _drain_workers(
        self,
        workers: list[asyncio.Task[Any]],
        *,
        on_cancel: Callable[[], asyncio.Task[None] | None] | None = None,
    ) -> list[Any]:
        """Wait for physical I/O, retaining ownership through repeated cancellation."""
        results: list[Any] = []
        errors: list[BaseException] = []
        cancellation_requested = False
        pending = list(workers)

        while pending:
            worker = pending.pop(0)
            try:
                results.append(await asyncio.shield(worker))
            except asyncio.CancelledError:
                if worker.cancelled():
                    errors.append(asyncio.CancelledError())
                    continue

                cancellation_requested = True
                current_task = asyncio.current_task()
                if current_task is not None:
                    while current_task.cancelling():
                        current_task.uncancel()
                if on_cancel is not None:
                    close_task = on_cancel()
                    on_cancel = None
                    if close_task is not None:
                        pending.append(close_task)
                pending.insert(0, worker)
            except BaseException as error:
                errors.append(error)

        if cancellation_requested:
            raise asyncio.CancelledError
        if errors:
            raise errors[0]
        return results
