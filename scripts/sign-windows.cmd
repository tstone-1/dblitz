@echo off
rem Tauri's sign command on the Windows release leg (src-tauri/tauri.signing.conf.json).
rem It is a .cmd because the uninstaller is signed from inside makensis, which
rem starts the command through the shell and from another directory. The work
rem is in sign-windows.ps1.
rem
rem The script is named by DBLITZ_SIGN_SCRIPT and not found beside this file:
rem makensis starts this file by its quoted name through PATH, and cmd then
rem gives the current folder for %~dp0. A wrapper that looked beside itself
rem ended with "UninstFinalize command returned 64", which makensis does not
rem treat as an error, and the installer was built with an unsigned uninstaller.
if not defined DBLITZ_SIGN_SCRIPT (
  echo DBLITZ_SIGN_SCRIPT is not set 1>&2
  exit /b 1
)
pwsh -NoProfile -ExecutionPolicy Bypass -File "%DBLITZ_SIGN_SCRIPT%" %1
exit /b %ERRORLEVEL%
