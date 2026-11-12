@echo off
REM === RTK Windows Build Script ===
REM Prerequisites:
REM   1. Rust 1.91+ (rustup: https://rustup.rs)
REM   2. Visual Studio 2022 Build Tools with "Desktop development with C++" workload
REM      OR Windows SDK 10.0.26100+ with rc.exe in PATH
REM
REM If link.exe fails with "extra operand" or "rc.exe not found":
REM   - Install Visual Studio 2022 Build Tools
REM   - OR ensure Windows SDK tools (rc.exe, link.exe) are in PATH
REM
REM Build output: target\release\rtk.exe

echo === RTK Windows Build ===
echo.

REM Clean previous build artifacts
echo [1/4] Cleaning previous build...
if exist target rmdir /s /q target
if exist D:\rtk-build rmdir /s /q D:\rtk-build
echo Done.

REM Use a build directory without spaces (avoids linker path issues)
echo [2/4] Building release binary...
set CARGO_TARGET_DIR=D:\rtk-build
cargo build --release
if %ERRORLEVEL% NEQ 0 (
    echo.
    echo BUILD FAILED. See errors above.
    echo.
    echo Common fixes:
    echo   1. Install Visual Studio 2022 Build Tools with C++ workload
    echo   2. Ensure Windows SDK is in PATH
    echo   3. Try: rustup update stable
    echo   4. Try: rustup default stable-msvc
    echo.
    exit /b 1
)

REM Copy binary to workspace root
echo [3/4] Copying rtk.exe to workspace root...
copy /Y D:\rtk-build\release\rtk.exe "D:\AI\RTK fuben\rtk.exe"
echo Done.

REM Verify binary
echo [4/4] Verifying binary...
"D:\AI\RTK fuben\rtk.exe" --version
if %ERRORLEVEL% NEQ 0 (
    echo Binary verification FAILED
    exit /b 1
)

echo.
echo === Build Complete ===
echo Binary: D:\AI\RTK fuben\rtk.exe
echo.
echo Next: test with hooks/claude/test-rtk-rewrite.ps1
exit /b 0
