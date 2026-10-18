@echo off
rem Starts OpenRemote, or restarts it if it is already running.
rem Works from this folder, from a shortcut, or from a copy on the desktop.
rem Only this build's exe is stopped (the optional SYSTEM input helper is left alone).
setlocal
set "EXE=%~dp0target\release\openremote.exe"
if not exist "%EXE%" set "EXE=C:\Users\Amogh\Projects\Remote Mouse\remote-mouse-rs\target\release\openremote.exe"

if not exist "%EXE%" (
  echo Cannot find openremote.exe:
  echo   %EXE%
  echo Build it first with:  cargo build --release
  pause
  exit /b 1
)

powershell -NoProfile -Command "Get-Process openremote -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $env:EXE } | Stop-Process -Force"
ping -n 2 127.0.0.1 >nul

start "" "%EXE%"

rem Give it a moment, then make sure it is really running instead of failing silently.
ping -n 3 127.0.0.1 >nul
powershell -NoProfile -Command "if (Get-Process openremote -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $env:EXE }) { exit 0 } else { exit 1 }"
if errorlevel 1 (
  echo OpenRemote did not start. To see why, run this in a terminal:
  echo   "%EXE%" --no-tray
  pause
  exit /b 1
)
