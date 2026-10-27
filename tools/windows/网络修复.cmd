@echo off
rem Network repair (first-aid): clears leftover VPN DNS hijack / adapter residue.
rem Double-click this if the internet is slow or web pages will not open after
rem using the VPN. Keep this file pure ASCII (cmd.exe parses .cmd as GBK).
set "PY="
if exist "%LOCALAPPDATA%\Programs\Python\Python313\python.exe" set "PY=%LOCALAPPDATA%\Programs\Python\Python313\python.exe"
if not defined PY for /f "delims=" %%i in ('py -3 -c "import sys;print(sys.executable)" 2^>nul') do set "PY=%%i"
if not defined PY for /f "delims=" %%i in ('python -c "import sys;print(sys.executable)" 2^>nul') do set "PY=%%i"
if not defined PY goto nopython
"%PY%" "%~dp0netfix.py"
pause
exit /b 0
:nopython
echo [ERROR] Python 3 not found.
echo Please install Python from python.org ^(any 3.x^), then run this file again.
pause
exit /b 1
