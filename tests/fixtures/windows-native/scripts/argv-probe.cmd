@echo off
setlocal EnableExtensions DisableDelayedExpansion
set /a idx=0
:loop
if "%~1"=="" goto done
echo arg%idx%=%~1
set /a idx+=1
shift
goto loop
:done
