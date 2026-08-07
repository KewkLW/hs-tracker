@echo off
rem Dev run with the MSVC environment (rustc needs vcvars to link).
rem Keep this file pure ASCII - non-ASCII bytes desync the cmd parser.
cd /d %~dp0
call "C:\Program Files (x86)\Microsoft Visual Studio\2019\BuildTools\VC\Auxiliary\Build\vcvars64.bat" >nul
set PATH=%USERPROFILE%\.cargo\bin;%PATH%
call npx tauri dev %*
