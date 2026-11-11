@echo off
title QuestUTSync - Vive Ultimate Tracker Auto-Sync
cd /d "%~dp0"
if exist "tracker_setup_lite.exe" (
    "tracker_setup_lite.exe" %*
) else if exist "dist\tracker_setup_lite.exe" (
    "dist\tracker_setup_lite.exe" %*
) else if exist "bin\Release\net8.0\win-x64\tracker_setup_lite.exe" (
    "bin\Release\net8.0\win-x64\tracker_setup_lite.exe" %*
) else (
    dotnet run -c Release -- %*
)
pause
