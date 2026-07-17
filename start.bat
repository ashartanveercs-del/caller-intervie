@echo off
REM One-click launcher for the interview copilot.
REM Double-click this file, or run it from a terminal.
cd /d "%~dp0"
start "" ".venv\Scripts\pythonw.exe" -m ai_assistant.main
