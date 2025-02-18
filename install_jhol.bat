@echo off
setlocal EnableDelayedExpansion

:: Set the installation directory
set "INSTALL_DIR=C:\Program Files\Jhol"

:: Create the directory if it doesn't exist
if not exist "%INSTALL_DIR%" mkdir "%INSTALL_DIR%"

:: Move the Jhol executable
copy /Y ".\target\release\jhol.exe" "%INSTALL_DIR%\"

:: Check if Jhol is already in PATH
for /f "tokens=2 delims=;" %%A in ('reg query HKCU\Environment /v Path 2^>nul') do set "OLD_PATH=%%A"

echo %OLD_PATH% | findstr /I /C:"%INSTALL_DIR%" >nul
if %errorlevel% neq 0 (
    echo Adding Jhol to system PATH...
    set "NEW_PATH=%INSTALL_DIR%;%OLD_PATH%"
    reg add HKCU\Environment /v Path /t REG_EXPAND_SZ /d "%NEW_PATH%" /f
    echo Restart your terminal or log out and back in to use 'jhol' globally.
) else (
    echo Jhol is already in PATH.
)

echo Installation complete. You can now run Jhol using: jhol --version
pause
