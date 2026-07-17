"""Microphone capture using sounddevice with async iteration.

Supports single-mic and dual-mic (your mic + system audio) modes.
System audio capture uses WASAPI loopback via PyAudioWPatch.
"""

from __future__ import annotations

import asyncio
import logging
import queue
import struct
import threading
from typing import AsyncIterator

import sounddevice as sd

logger = logging.getLogger(__name__)


_LOOPBACK_KEYWORDS = {"stereo", "mix", "loopback", "virtual", "cable", "what u hear", "wave out"}
_MIC_KEYWORDS = {"mikrofon", "microphone", "mic", "headset", "headphone", "airpod", "input", "capture"}


def list_input_devices() -> list[dict]:
    """Return all audio input devices."""
    devices = []
    for i, d in enumerate(sd.query_devices()):
        if d["max_input_channels"] > 0:
            devices.append({
                "index": i,
                "name": d["name"],
                "channels": d["max_input_channels"],
                "sample_rate": d["default_samplerate"],
            })
    return devices


def list_mic_devices() -> list[dict]:
    """Return input devices that are likely real microphones."""
    devices = []
    for d in list_input_devices():
        name_lower = d["name"].lower()
        # Exclude known loopback/output-capture devices
        if any(kw in name_lower for kw in _LOOPBACK_KEYWORDS):
            continue
        devices.append(d)
    return devices


def list_output_capture_devices() -> list[dict]:
    """Return input devices that capture system output (Stereomix, loopback, etc)."""
    devices = []
    for d in list_input_devices():
        name_lower = d["name"].lower()
        if any(kw in name_lower for kw in _LOOPBACK_KEYWORDS):
            devices.append(d)
    return devices


class MicCapture:
    """Captures raw audio from a microphone and yields bytes chunks.

    The sounddevice callback runs on a C thread, so it only touches a
    thread-safe ``queue.Queue``.  The async ``__aiter__`` drains that queue
    from the event-loop thread via ``run_in_executor``.
    """

    def __init__(
        self,
        sample_rate: int = 16000,
        blocksize: int = 1024,
        channels: int = 1,
        dtype: str = "int16",
        device: int | None = None,
    ) -> None:
        self._sample_rate = sample_rate
        self._blocksize = blocksize
        self._channels = channels
        self._dtype = dtype
        self._device = device  # None = system default
        self._sync_queue: queue.Queue[bytes | None] = queue.Queue(maxsize=100)
        self._stream: sd.RawInputStream | None = None

    # ------------------------------------------------------------------
    # sounddevice callback — runs on a C audio thread
    # ------------------------------------------------------------------
    def _audio_callback(
        self,
        indata: bytes,
        frames: int,
        time_info: object,
        status: sd.CallbackFlags,
    ) -> None:
        """Put raw audio bytes onto the queue; drop frames on backpressure."""
        if status:
            logger.warning("sounddevice status: %s", status)
        try:
            self._sync_queue.put_nowait(bytes(indata))
        except queue.Full:
            logger.warning("Audio queue full — dropping frame (%d frames)", frames)

    # ------------------------------------------------------------------
    # Lifecycle
    # ------------------------------------------------------------------
    async def start(self) -> None:
        """Create and start the ``RawInputStream``."""
        logger.info(
            "Starting mic capture: rate=%d blocksize=%d channels=%d dtype=%s",
            self._sample_rate,
            self._blocksize,
            self._channels,
            self._dtype,
        )
        self._stream = sd.RawInputStream(
            samplerate=self._sample_rate,
            blocksize=self._blocksize,
            channels=self._channels,
            dtype=self._dtype,
            device=self._device,
            callback=self._audio_callback,
        )
        self._stream.start()
        logger.info("Mic capture started")

    async def stop(self) -> None:
        """Stop the stream and send the sentinel so ``__aiter__`` exits."""
        if self._stream is not None:
            self._stream.stop()
            self._stream.close()
            self._stream = None
            logger.info("Mic stream stopped")
        # Sentinel tells the async iterator to finish.
        self._sync_queue.put(None)

    async def change_device(self, device: int | None) -> None:
        """Switch to a different input device (restarts the stream)."""
        was_running = self._stream is not None
        if was_running:
            self._stream.stop()
            self._stream.close()
            self._stream = None
        self._device = device
        if was_running:
            await self.start()
        logger.info("Switched mic to device %s", device)

    # ------------------------------------------------------------------
    # Async iteration
    # ------------------------------------------------------------------
    async def __aiter__(self) -> AsyncIterator[bytes]:
        """Yield audio chunks until a ``None`` sentinel is received."""
        loop = asyncio.get_running_loop()
        while True:
            chunk = await loop.run_in_executor(None, self._sync_queue.get)
            if chunk is None:
                logger.debug("Received stop sentinel — ending mic iteration")
                break
            yield chunk


class DualMicCapture:
    """Captures from two audio devices and mixes them into one stream.

    Use this to capture your microphone + system audio (Stereomix/loopback)
    so Deepgram can transcribe both sides of a call.
    """

    def __init__(
        self,
        sample_rate: int = 16000,
        blocksize: int = 1024,
        dtype: str = "int16",
        mic_device: int | None = None,
        system_device: int | None = None,
    ) -> None:
        self._sample_rate = sample_rate
        self._blocksize = blocksize
        self._dtype = dtype
        self._mic_device = mic_device
        self._system_device = system_device
        # Yields (source, bytes) tuples — source is "mic" or "system"
        self._sync_queue: queue.Queue[tuple[str, bytes] | None] = queue.Queue(maxsize=200)
        self._mic_stream: sd.RawInputStream | None = None
        self._sys_thread: threading.Thread | None = None
        self._sys_running = False

    def _mic_callback(self, indata: bytes, frames: int, time_info: object, status: sd.CallbackFlags) -> None:
        if status:
            logger.warning("Mic status: %s", status)
        try:
            self._sync_queue.put_nowait(("mic", bytes(indata)))
        except queue.Full:
            pass

    def _sys_pyaudio_thread(self) -> None:
        """Background thread capturing system audio via WASAPI loopback."""
        try:
            import pyaudiowpatch as pyaudio
            p = pyaudio.PyAudio()
            wasapi = p.get_host_api_info_by_type(pyaudio.paWASAPI)
            default_out = p.get_device_info_by_index(wasapi["defaultOutputDevice"])

            # Find loopback device for default output
            loopback_dev = None
            for i in range(p.get_device_count()):
                d = p.get_device_info_by_index(i)
                if d.get("isLoopbackDevice") and default_out["name"].split("(")[0].strip() in d["name"]:
                    loopback_dev = d
                    break

            if loopback_dev is None:
                logger.error("No WASAPI loopback device found")
                p.terminate()
                return

            logger.info("WASAPI loopback: %s (rate=%s)", loopback_dev["name"], loopback_dev["defaultSampleRate"])

            native_rate = int(loopback_dev["defaultSampleRate"])
            channels = loopback_dev["maxInputChannels"]

            stream = p.open(
                format=pyaudio.paInt16,
                channels=channels,
                rate=native_rate,
                input=True,
                input_device_index=loopback_dev["index"],
                frames_per_buffer=self._blocksize,
            )

            while self._sys_running:
                data = stream.read(self._blocksize, exception_on_overflow=False)
                # Convert to mono 16kHz if needed
                pcm = self._resample_to_mono_16k(data, native_rate, channels)
                try:
                    self._sync_queue.put_nowait(("system", pcm))
                except queue.Full:
                    pass

            stream.stop_stream()
            stream.close()
            p.terminate()
            logger.info("WASAPI loopback stopped")

        except Exception:
            logger.exception("WASAPI loopback capture failed")

    @staticmethod
    def _resample_to_mono_16k(data: bytes, src_rate: int, src_channels: int) -> bytes:
        """Downsample multi-channel audio to mono 16kHz 16-bit PCM."""
        n_samples = len(data) // (2 * src_channels)
        samples = struct.unpack(f"<{n_samples * src_channels}h", data)

        # Mix to mono by averaging channels
        mono = []
        for i in range(0, len(samples), src_channels):
            mono.append(sum(samples[i:i + src_channels]) // src_channels)

        # Downsample: take every Nth sample (simple decimation)
        ratio = src_rate / 16000
        resampled = []
        pos = 0.0
        while int(pos) < len(mono):
            resampled.append(mono[int(pos)])
            pos += ratio

        return struct.pack(f"<{len(resampled)}h", *resampled)

    async def start(self) -> None:
        if self._mic_device is not None:
            logger.info("Starting mic capture (device %s)", self._mic_device)
            self._mic_stream = sd.RawInputStream(
                samplerate=self._sample_rate,
                blocksize=self._blocksize,
                channels=1,
                dtype=self._dtype,
                device=self._mic_device,
                callback=self._mic_callback,
            )
            self._mic_stream.start()

        logger.info("Dual mic capture started")

    def start_system_capture(self) -> None:
        """Start WASAPI loopback capture in a background thread."""
        if self._sys_thread is not None and self._sys_thread.is_alive():
            self.stop_system_capture()
        self._sys_running = True
        self._sys_thread = threading.Thread(target=self._sys_pyaudio_thread, daemon=True)
        self._sys_thread.start()
        logger.info("System audio capture started (WASAPI loopback)")

    def stop_system_capture(self) -> None:
        self._sys_running = False
        if self._sys_thread is not None:
            self._sys_thread.join(timeout=3)
            self._sys_thread = None

    async def stop(self) -> None:
        if self._mic_stream is not None:
            self._mic_stream.stop()
            self._mic_stream.close()
        self._mic_stream = None
        self.stop_system_capture()
        self._sync_queue.put(None)
        logger.info("Dual mic capture stopped")

    async def change_mic_device(self, device: int | None) -> None:
        if self._mic_stream is not None:
            self._mic_stream.stop()
            self._mic_stream.close()
            self._mic_stream = None
        self._mic_device = device
        if device is not None:
            self._mic_stream = sd.RawInputStream(
                samplerate=self._sample_rate,
                blocksize=self._blocksize,
                channels=1,
                dtype=self._dtype,
                device=device,
                callback=self._mic_callback,
            )
            self._mic_stream.start()
        logger.info("Switched mic to device %s", device)

    async def change_system_device(self, device: int | None) -> None:
        """Toggle system audio capture on/off."""
        if device is not None:
            self.start_system_capture()
        else:
            self.stop_system_capture()

    async def __aiter__(self) -> AsyncIterator[tuple[str, bytes]]:
        """Yields (source, audio_bytes) tuples."""
        loop = asyncio.get_running_loop()
        while True:
            item = await loop.run_in_executor(None, self._sync_queue.get)
            if item is None:
                break
            yield item
