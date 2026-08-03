"""Focused tests for the Qt-to-runtime shutdown adapter."""

import asyncio
import threading

import ai_assistant.main as main


class _FakeSignal:
    def __init__(self) -> None:
        self.handlers = []

    def connect(self, handler) -> None:
        self.handlers.append(handler)

    def emit(self) -> None:
        for handler in list(self.handlers):
            handler()


class _FakeApplication:
    def __init__(self) -> None:
        self.aboutToQuit = _FakeSignal()


class _FakeRuntime:
    def __init__(self) -> None:
        self.stop_calls = 0

    async def stop_session(self) -> None:
        self.stop_calls += 1


def test_qt_about_to_quit_awaits_runtime_shutdown() -> None:
    async def scenario() -> _FakeRuntime:
        application = _FakeApplication()
        runtime = _FakeRuntime()
        shutdown_task = asyncio.create_task(
            main._run_until_application_quit(application, runtime)
        )
        await asyncio.sleep(0)

        emitter = threading.Thread(target=application.aboutToQuit.emit)
        emitter.start()
        emitter.join(timeout=1)
        await asyncio.wait_for(shutdown_task, timeout=1)
        return runtime

    runtime = asyncio.run(scenario())
    assert runtime.stop_calls == 1
