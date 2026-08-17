"""Focused tests for the Qt-to-runtime shutdown adapter."""

import asyncio
import json
import logging
import os
from pathlib import Path
import subprocess
import sys
import textwrap
import threading
import types

import pytest

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


def test_runtime_import_error_is_not_reported_as_missing_qasync(
    monkeypatch, caplog
) -> None:
    class FakeApplication:
        @classmethod
        def instance(cls):
            return None

        def __init__(self, _arguments) -> None:
            pass

        def setQuitOnLastWindowClosed(self, _enabled: bool) -> None:
            pass

    class FakeLoop:
        def __init__(self, _application) -> None:
            pass

        def __enter__(self):
            return self

        def __exit__(self, _exc_type, _exc_value, _traceback) -> None:
            pass

        def run_until_complete(self, awaitable) -> None:
            asyncio.run(awaitable)

    async def failing_async_main() -> None:
        raise ImportError("runtime dependency unavailable")

    qasync = types.ModuleType("qasync")
    qasync.QEventLoop = FakeLoop
    qt_widgets = types.ModuleType("PySide6.QtWidgets")
    qt_widgets.QApplication = FakeApplication
    monkeypatch.setitem(sys.modules, "qasync", qasync)
    monkeypatch.setitem(sys.modules, "PySide6.QtWidgets", qt_widgets)
    monkeypatch.setattr(main, "_setup_logging", lambda: None)
    monkeypatch.setattr(main, "load_dotenv", lambda: None)
    monkeypatch.setattr(main.asyncio, "set_event_loop", lambda _loop: None)
    monkeypatch.setattr(main, "async_main", failing_async_main)

    with caplog.at_level(logging.ERROR, logger=main.__name__):
        with pytest.raises(ImportError, match="runtime dependency unavailable"):
            main.main()

    assert "qasync is required" not in caplog.text


def test_real_tray_quit_drains_runtime_before_qt_exits() -> None:
    script = textwrap.dedent(
        """
        import asyncio
        import json

        from PySide6.QtCore import QTimer
        from PySide6.QtWidgets import QApplication
        import qasync

        import ai_assistant.main as main
        from ai_assistant.ui.signals import AppSignals
        from ai_assistant.ui.tray_icon import TrayIcon


        order = []


        class Runtime:
            async def stop_session(self):
                order.append("runtime_stopped")


        application = QApplication([])
        application.setQuitOnLastWindowClosed(False)
        application.aboutToQuit.connect(lambda: order.append("application_exited"))
        signals = AppSignals()
        tray = TrayIcon(signals)
        quit_action = next(
            action for action in tray.contextMenu().actions() if action.text() == "Quit"
        )

        loop = qasync.QEventLoop(application)
        asyncio.set_event_loop(loop)


        async def scenario():
            shutdown_event = main._application_shutdown_event(application)
            if hasattr(signals, "quit_requested"):
                signals.quit_requested.connect(shutdown_event.set)
            QTimer.singleShot(0, quit_action.trigger)
            try:
                await main._run_until_application_quit(
                    application, Runtime(), shutdown_event
                )
            finally:
                order.append("ui_cleaned_up")


        with loop:
            loop.run_until_complete(scenario())

        print("RESULT=" + json.dumps(order))
        """
    )
    environment = os.environ.copy()
    environment["QT_QPA_PLATFORM"] = "offscreen"
    result = subprocess.run(
        [sys.executable, "-c", script],
        cwd=Path(__file__).resolve().parents[3],
        env=environment,
        capture_output=True,
        text=True,
        timeout=15,
        check=False,
    )

    assert result.returncode == 0, result.stdout + result.stderr
    result_line = next(
        line for line in result.stdout.splitlines() if line.startswith("RESULT=")
    )
    order = json.loads(result_line.removeprefix("RESULT="))
    assert order == ["runtime_stopped", "ui_cleaned_up", "application_exited"]
