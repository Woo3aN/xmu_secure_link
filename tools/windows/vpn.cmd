@echo off
rem XMU SecureLink one-click launcher (portable, copy whole folder to use)
rem NOTE: keep this file pure ASCII. cmd.exe parses .cmd as GBK; any
rem non-ASCII byte here will garble the script and break execution.
rem NOTE: "if not defined PY for /f ..." - do NOT wrap in ( ) on one line:
rem the ( ) inside the py command breaks cmd parenthesis pairing.
set "PY="
if exist "%LOCALAPPDATA%\Programs\Python\Python313\python.exe" set "PY=%LOCALAPPDATA%\Programs\Python\Python313\python.exe"
if not defined PY for /f "delims=" %%i in ('py -3 -c "import sys;print(sys.executable)" 2^>nul') do set "PY=%%i"
if not defined PY for /f "delims=" %%i in ('python -c "import sys;print(sys.executable)" 2^>nul') do set "PY=%%i"
if not defined PY goto nopython
"%PY%" "%~dp0vpn_start.py"
pause
exit /b 0
:nopython
echo [ERROR] Python 3 not found.
echo Please install Python from python.org ^(any 3.x^), then run this file again.
pause
exit /b 1
