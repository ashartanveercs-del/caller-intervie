"""Behavioral tests for the UI-independent assistant runtime."""

from __future__ import annotations

import asyncio
from dataclasses import dataclass
from pathlib import Path
import subprocess
import sys

import pytest

from ai_assistant.config import Config
from ai_assistant.core.events import EventBus
from ai_assistant.runtime import (
    RuntimeConfigurationError,
    RuntimeService,
    RuntimeStateError,
    SessionConfig,
    build_runtime,
)


def _run(coro):
    return asyncio.run(coro)


async def _wait_for_knowledge(service: RuntimeService) -> None:
    for _ in range(50):
        if service.snapshot().knowledge_state != "loading":
            return
        await asyncio.sleep(0.01)
    raise TimeoutError("knowledge load did not settle")


def _session(**overrides: str) -> SessionConfig:
    values = {
        "session_id": "018f0000-0000-7000-8000-000000000003",
        "mode": "interview",
        "input_language": "auto",
        "response_language": "en",
        "review_language": "en",
        "you_source": "mic",
        "brief_id": "018f0000-0000-7000-8000-000000000010",
    }
    values.update(overrides)
    return SessionConfig(**values)


class FakeCapture:
    def __init__(self) -> None:
        self.started = False
        self.start_calls = 0
        self.stop_calls = 0
        self.system_start_calls = 0
        self.system_stop_calls = 0
        self.mic_devices: list[int | None] = []
        self.system_devices: list[int | None] = []
        self.iteration_error: Exception | None = None
        self.stop_entered: asyncio.Event | None = None
        self.stop_release: asyncio.Event | None = None
        self._chunks: asyncio.Queue[tuple[str, bytes]] = asyncio.Queue()

    async def start(self) -> None:
        self.start_calls += 1
        self.started = True

    async def stop(self) -> None:
        self.stop_calls += 1
        if self.stop_entered is not None:
            self.stop_entered.set()
        if self.stop_release is not None:
            await self.stop_release.wait()
        self.started = False

    def start_system_capture(self) -> None:
        self.system_start_calls += 1

    def stop_system_capture(self) -> None:
        self.system_stop_calls += 1

    async def change_mic_device(self, device: int | None) -> None:
        self.mic_devices.append(device)

    async def change_system_device(self, device: int | None) -> None:
        self.system_devices.append(device)

    async def push(self, source: str, chunk: bytes) -> None:
        await self._chunks.put((source, chunk))

    async def __aiter__(self):
        if self.iteration_error is not None:
            raise self.iteration_error
        while True:
            yield await self._chunks.get()


class FakeTranscriber:
    def __init__(self, config: Config) -> None:
        self.config = config
        self.started = False
        self.start_calls = 0
        self.stop_calls = 0
        self.system_start_calls = 0
        self.system_stop_calls = 0
        self.start_languages: tuple[str, str, str] | None = None
        self.mic_audio: list[bytes] = []
        self.system_audio: list[bytes] = []
        self.start_error: Exception | None = None

    async def start(self) -> None:
        self.start_calls += 1
        self.started = True
        self.start_languages = (
            self.config.deepgram_language,
            self.config.response_language,
            self.config.review_language,
        )
        if self.start_error is not None:
            raise self.start_error

    async def stop(self) -> None:
        self.stop_calls += 1
        self.started = False

    async def start_system_stream(self) -> None:
        self.system_start_calls += 1

    async def stop_system_stream(self) -> None:
        self.system_stop_calls += 1

    async def send_mic_audio(self, data: bytes) -> None:
        self.mic_audio.append(data)

    async def send_system_audio(self, data: bytes) -> None:
        self.system_audio.append(data)


class FakePromptBuilder:
    def __init__(self) -> None:
        self.custom_system_prompt = ""
        self.history: list[tuple[str, str]] = []

    def add_to_history(self, role: str, content: str) -> None:
        self.history.append((role, content))


class FakeLLM:
    def __init__(self) -> None:
        self.cancel_calls = 0

    def cancel_active(self) -> None:
        self.cancel_calls += 1


class FakeTranscriptBuffer:
    def __init__(self, text: str = "") -> None:
        self.text = text

    def get_full_text(self) -> str:
        return self.text


class FakeOrchestrator:
    def __init__(self, transcript: str = "") -> None:
        self.listening = True
        self.mode = "suggestion"
        self.you_source = "mic"
        self.retriever = None
        self.query_calls: list[tuple[str | None, bool]] = []
        self.cancel_pending_calls = 0
        self._transcript_buffer = FakeTranscriptBuffer(transcript)

    def set_listening(self, enabled: bool) -> None:
        self.listening = enabled

    def set_you_source(self, source: str) -> None:
        self.you_source = source

    def set_retriever(self, retriever) -> None:
        self.retriever = retriever

    async def set_mode(self, mode) -> None:
        self.mode = mode

    async def trigger_query(
        self,
        explicit_query: str | None = None,
        full_transcript: bool = False,
    ) -> None:
        self.query_calls.append((explicit_query, full_transcript))

    def cancel_pending(self) -> None:
        self.cancel_pending_calls += 1


class FakeVectorStore:
    def __init__(self) -> None:
        self.size = 0


class FakeRetriever:
    def __init__(self) -> None:
        self.vector_store = FakeVectorStore()
        self.ingested: list[str] = []
        self.saved_to: list[str] = []

    def ingest_file(self, path: str) -> int:
        self.ingested.append(path)
        self.vector_store.size += 2
        return 2

    def save_index(self, path: str) -> None:
        self.saved_to.append(path)


@dataclass
class FakeRuntimeDependencies:
    config: Config
    event_bus: EventBus
    capture: FakeCapture
    transcriber: FakeTranscriber
    llm: FakeLLM
    prompt_builder: FakePromptBuilder
    orchestrator: FakeOrchestrator
    rag_factory: object = None


def _dependencies(*, transcript: str = "") -> FakeRuntimeDependencies:
    config = Config(deepgram_api_key="dg", llm_api_key="llm")
    return FakeRuntimeDependencies(
        config=config,
        event_bus=EventBus(),
        capture=FakeCapture(),
        transcriber=FakeTranscriber(config),
        llm=FakeLLM(),
        prompt_builder=FakePromptBuilder(),
        orchestrator=FakeOrchestrator(transcript),
    )


def test_runtime_starts_without_ui_and_routes_audio() -> None:
    async def scenario() -> tuple[RuntimeService, FakeRuntimeDependencies]:
        deps = _dependencies()
        service = RuntimeService(deps)
        await service.start_session(_session())
        await deps.capture.push("mic", b"mic-pcm")
        await deps.capture.push("system", b"system-pcm")
        await asyncio.sleep(0)
        await asyncio.sleep(0)
        return service, deps

    service, deps = _run(scenario())
    assert service.snapshot().state == "listening"
    assert deps.capture.started is True
    assert deps.transcriber.started is True
    assert deps.transcriber.mic_audio == [b"mic-pcm"]
    assert deps.transcriber.system_audio == [b"system-pcm"]
    _run(service.stop_session())


def test_missing_credentials_returns_configuration_error_before_start() -> None:
    async def scenario() -> list[str]:
        service = build_runtime(Config(deepgram_api_key="", llm_api_key=""))
        with pytest.raises(RuntimeConfigurationError) as exc:
            await service.start_session(_session())
        assert service.snapshot().state == "idle"
        return exc.value.missing

    assert _run(scenario()) == ["deepgram_api_key", "llm_api_key"]


def test_stop_is_idempotent_and_stops_each_dependency_once() -> None:
    async def scenario() -> tuple[RuntimeService, FakeRuntimeDependencies]:
        deps = _dependencies()
        service = RuntimeService(deps)
        await service.start_session(_session())
        await service.stop_session()
        await service.stop_session()
        return service, deps

    service, deps = _run(scenario())
    assert service.snapshot().state == "stopped"
    assert deps.capture.stop_calls == 1
    assert deps.transcriber.stop_calls == 1


def test_failed_audio_pump_does_not_skip_remaining_cleanup() -> None:
    async def scenario() -> tuple[RuntimeService, FakeRuntimeDependencies, FakeRetriever]:
        retriever = FakeRetriever()
        deps = _dependencies()
        deps.capture.iteration_error = RuntimeError("audio pump failed")
        deps.rag_factory = lambda: retriever
        service = RuntimeService(deps)
        await service.start_session(_session())
        await _wait_for_knowledge(service)
        await asyncio.sleep(0)

        with pytest.raises(RuntimeError, match="audio pump failed"):
            await service.stop_session()
        return service, deps, retriever

    service, deps, retriever = _run(scenario())
    assert deps.capture.stop_calls == 1
    assert deps.transcriber.stop_calls == 1
    assert retriever.saved_to == [deps.config.rag_db_path]
    assert service.snapshot().state == "error"


def test_failed_partial_start_is_cleaned_up_and_stop_remains_idempotent() -> None:
    async def scenario() -> tuple[RuntimeService, FakeRuntimeDependencies]:
        deps = _dependencies()
        deps.transcriber.start_error = RuntimeError("provider start failed")
        service = RuntimeService(deps)
        with pytest.raises(RuntimeError, match="provider start failed"):
            await service.start_session(_session())
        await service.stop_session()
        return service, deps

    service, deps = _run(scenario())
    assert deps.transcriber.stop_calls == 1
    assert deps.capture.start_calls == 0
    assert deps.capture.stop_calls == 0
    assert deps.orchestrator.listening is False
    assert deps.orchestrator.cancel_pending_calls == 1
    assert service.snapshot().state == "error"


def test_stopping_blocks_new_async_operations_and_cancels_orchestrator() -> None:
    async def scenario() -> tuple[RuntimeService, FakeRuntimeDependencies, FakeRetriever]:
        retriever = FakeRetriever()
        deps = _dependencies()
        deps.rag_factory = lambda: retriever
        deps.capture.stop_entered = asyncio.Event()
        deps.capture.stop_release = asyncio.Event()
        service = RuntimeService(deps)
        await service.start_session(_session())
        await _wait_for_knowledge(service)

        stop_task = asyncio.create_task(service.stop_session())
        await deps.capture.stop_entered.wait()
        operation_tasks = [
            asyncio.create_task(service.set_system_audio(True)),
            asyncio.create_task(service.set_audio_device("mic", 9)),
            asyncio.create_task(service.trigger_query("late query", "detail")),
            asyncio.create_task(service.ingest_documents(["late.txt"])),
        ]
        await asyncio.sleep(0)

        assert service.snapshot().state == "stopping"
        assert deps.capture.system_start_calls == 0
        assert deps.capture.mic_devices == []
        assert deps.orchestrator.query_calls == []
        assert retriever.ingested == []

        deps.capture.stop_release.set()
        await stop_task
        for task in operation_tasks:
            with pytest.raises(RuntimeStateError, match="No runtime session is active"):
                await task
        return service, deps, retriever

    service, deps, retriever = _run(scenario())
    assert service.snapshot().state == "stopped"
    assert deps.orchestrator.cancel_pending_calls == 1
    assert deps.capture.system_start_calls == 0
    assert deps.capture.mic_devices == []
    assert deps.orchestrator.query_calls == []
    assert retriever.ingested == []


def test_device_and_role_switching_use_public_runtime_controls() -> None:
    async def scenario() -> tuple[RuntimeService, FakeRuntimeDependencies]:
        deps = _dependencies()
        service = RuntimeService(deps)
        await service.start_session(_session())
        await service.set_audio_device("mic", 7)
        await service.set_audio_device("system", 11)
        service.set_you_source("system")
        return service, deps

    service, deps = _run(scenario())
    assert deps.capture.mic_devices == [7]
    assert deps.capture.system_devices == [11]
    assert deps.orchestrator.you_source == "system"
    assert service.snapshot().you_source == "system"
    _run(service.stop_session())


def test_system_audio_switching_is_idempotent() -> None:
    async def scenario() -> tuple[RuntimeService, FakeRuntimeDependencies]:
        deps = _dependencies()
        service = RuntimeService(deps)
        await service.start_session(_session())
        await service.set_system_audio(True)
        await service.set_system_audio(True)
        await service.set_system_audio(False)
        await service.set_system_audio(False)
        return service, deps

    service, deps = _run(scenario())
    assert deps.capture.system_start_calls == 1
    assert deps.transcriber.system_start_calls == 1
    assert deps.capture.system_stop_calls == 1
    assert deps.transcriber.system_stop_calls == 1
    assert service.snapshot().system_audio_enabled is False
    _run(service.stop_session())


def test_session_languages_reach_shared_provider_configuration() -> None:
    async def scenario() -> FakeRuntimeDependencies:
        deps = _dependencies()
        service = RuntimeService(deps)
        await service.start_session(
            _session(
                input_language="fr",
                response_language="de",
                review_language="es",
            )
        )
        await service.stop_session()
        return deps

    deps = _run(scenario())
    assert deps.transcriber.start_languages == ("fr", "de", "es")


def test_listening_query_and_transcript_are_exposed_without_dependencies() -> None:
    async def scenario() -> tuple[RuntimeService, FakeRuntimeDependencies]:
        deps = _dependencies(transcript="Interviewer: Tell me about yourself.")
        service = RuntimeService(deps)
        await service.start_session(_session())
        service.set_listening(False)
        await service.trigger_query("Summarize", "summary")
        return service, deps

    service, deps = _run(scenario())
    assert service.snapshot().state == "paused"
    assert deps.orchestrator.query_calls == [("Summarize", True)]
    assert service.full_transcript() == "Interviewer: Tell me about yourself."
    _run(service.stop_session())


def test_ingest_documents_uses_ready_retriever_and_updates_snapshot() -> None:
    async def scenario() -> tuple[RuntimeService, FakeRetriever]:
        retriever = FakeRetriever()
        deps = _dependencies()
        deps.rag_factory = lambda: retriever
        service = RuntimeService(deps)
        await service.start_session(_session())
        await _wait_for_knowledge(service)
        result = await service.ingest_documents(["resume.pdf", "role.txt"])
        assert result.files == 2
        assert result.chunks_added == 4
        assert result.total_chunks == 4
        assert service.snapshot().knowledge_state == "ready"
        await service.stop_session()
        return service, retriever

    service, retriever = _run(scenario())
    assert retriever.ingested == ["resume.pdf", "role.txt"]
    assert retriever.saved_to
    assert service.snapshot().knowledge_chunks == 4


def test_knowledge_load_failure_settles_without_blocking_shutdown() -> None:
    async def scenario() -> RuntimeService:
        deps = _dependencies()

        def fail_load():
            raise RuntimeError("index failed")

        deps.rag_factory = fail_load
        service = RuntimeService(deps)
        await service.start_session(_session())
        await asyncio.wait_for(_wait_for_knowledge(service), timeout=0.5)
        await service.stop_session()
        return service

    service = _run(scenario())
    assert service.snapshot().knowledge_state == "error"
    assert service.snapshot().state == "stopped"


def test_importing_runtime_never_attempts_to_import_pyside6() -> None:
    project_root = Path(__file__).resolve().parents[3]
    code = """
import builtins
original_import = builtins.__import__
def guarded_import(name, *args, **kwargs):
    if name == 'PySide6' or name.startswith('PySide6.'):
        raise AssertionError(f'PySide6 import attempted: {name}')
    return original_import(name, *args, **kwargs)
builtins.__import__ = guarded_import
import ai_assistant.runtime
"""
    completed = subprocess.run(
        [sys.executable, "-c", code],
        cwd=project_root,
        capture_output=True,
        text=True,
        check=False,
    )
    assert completed.returncode == 0, completed.stderr
