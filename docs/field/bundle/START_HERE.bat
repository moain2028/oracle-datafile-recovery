@echo off
rem ODR evaluator bundle - offline self-check. Double-click this file.
rem No internet, no GitHub account, no token: every file it needs is in this folder.
chcp 65001 >nul
setlocal
cd /d "%~dp0"
echo.
echo  ODR - Oracle Datafile Recovery - self-check (@TAG@)
echo  Runs 33 checks against real Oracle 26ai datafiles. About 1-2 minutes. Needs ~600 MB free disk.
echo.
echo  Step 0: integrity check (SHA-256 of every file + ODR signature)
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0integrity\verify.ps1"
if errorlevel 1 (
  echo  ==== STOP: this folder was changed or damaged. Download the bundle again.
  if not defined ODR_NO_PAUSE pause
  exit /b 1
)
rem "%~dp0." (with the dot): a trailing backslash before a closing quote would escape the quote.
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0try_odr.ps1" -Dir "%~dp0." -Tag @TAG@ -SkipDownload -LogFile "%~dp0odr-trial-log.txt"
set RC=%ERRORLEVEL%
echo.
if "%RC%"=="0" (
  echo  ==== PASS: all checks passed. Log: odr-trial-log.txt
) else (
  echo  ==== FAIL: send odr-trial-log.txt from this folder to the owner.
)
echo.
if not defined ODR_NO_PAUSE pause
exit /b %RC%
