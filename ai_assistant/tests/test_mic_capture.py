"""Production adapter tests for microphone startup failures."""

import asyncio

import pytest

from ai_assistant.audio import mic_capture
from ai_assistant.audio.mic_capture import DualMicCapture


def test_dual_mic_start_propagates_device_open_failure(monkeypatch) -> None:
    def fail_open(**_kwargs):
        raise RuntimeError("device unavailable")

    monkeypatch.setattr(mic_capture.sd, "RawInputStream", fail_open)
    capture = DualMicCapture()

    with pytest.raises(RuntimeError, match="device unavailable"):
        asyncio.run(capture.start())
