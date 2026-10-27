@echo off
rem XMU SecureLink TAP adapter repair tool - double-click entry (portable)
rem Keep this file pure ASCII (cmd.exe parses .cmd as GBK).
rem It only needs to be run if vpn.cmd reports TAP adapter problems.
set "PY="
if exist "%LOCALAPPDATA%\Programs\Python\Python313\python.exe" set "PY=%LOCALAPPDATA%\Programs\Python\Python313\python.exe"
if not defined PY for /f "delims=" %%i in ('py -3 -c "import sys;print(sys.executable)" 2^>nul') do set "PY=%%i"
if not defined PY for /f "delims=" %%i in ('python -c "import sys;print(sys.executable)" 2^>nul') do set "PY=%%i"
if not defined PY goto nopython
"%PY%" "%~dp0tap_repair.py"
pause
exit /b 0
:nopython
echo [ERROR] Python 3 not found.
echo Please install Python from python.org ^(any 3.x^), then run this file again.
pause
exit /b 1
