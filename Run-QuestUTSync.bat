@echo off
title QuestUTSync - Vive Ultimate Tracker Auto-Sync
cd /d "%~dp0"
if exist "tracker_setup_lite.exe" (
    "tracker_setup_lite.exe" %*
) else if exist "dist\tracker_setup_lite.exe" (
    "dist\tracker_setup_lite.exe" %*
) else if exist "target\release\tracker_setup_lite.exe" (
    "target\release\tracker_setup_lite.exe" %*
) else (
    cargo run --release -- %*
)
if errorlevel 1 pause
