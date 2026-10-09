@echo off
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0tools\test-core.ps1" %*
