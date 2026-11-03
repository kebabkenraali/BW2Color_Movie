# BW2Color Movie installer: copies the program to %LOCALAPPDATA%\Programs\BW2Color Movie (no administrator
# rights needed), creates Start-menu and desktop shortcuts and an uninstaller. An existing installation is
# updated in place: downloaded add-ons (FFmpeg, models) and settings are kept.
# -Target and -NoShortcuts are for testing the installer.
param([string]$Target = "", [switch]$NoShortcuts)
$ErrorActionPreference = "Stop"
$src = $PSScriptRoot
$dst = if ($Target) { $Target } else { Join-Path $env:LOCALAPPDATA "Programs\BW2Color Movie" }
Write-Host "Installing BW2Color Movie to $dst ..."
New-Item -ItemType Directory -Force $dst | Out-Null
Get-ChildItem $src -Force | Where-Object { $_.Name -notin @("Install.cmd", "install.ps1") } |
    Copy-Item -Destination $dst -Recurse -Force

$shell = New-Object -ComObject WScript.Shell
$exe = Join-Path $dst "BW2Color.exe"
$menu = Join-Path ([Environment]::GetFolderPath("Programs")) "BW2Color Movie.lnk"
$desk = Join-Path ([Environment]::GetFolderPath("Desktop")) "BW2Color Movie.lnk"
foreach ($l in $(if ($NoShortcuts) { @() } else { @($menu, $desk) })) {
    $s = $shell.CreateShortcut($l)
    $s.TargetPath = $exe
    $s.WorkingDirectory = $dst
    $s.Description = "BW2Color Movie - black-and-white film colorizer"
    $s.Save()
}

# uninstaller: removes the shortcuts and the installation folder (including downloaded add-ons and settings)
$un = @"
@echo off
echo This removes BW2Color Movie, its downloaded add-ons and settings from
echo   $dst
choice /m "Continue"
if errorlevel 2 exit /b
del "$menu" 2>nul
del "$desk" 2>nul
cd /d "%TEMP%"
start "" /b cmd /c "timeout /t 2 >nul & rmdir /s /q ""$dst"""
"@
Set-Content -Path (Join-Path $dst "Uninstall.cmd") -Value $un -Encoding ASCII
Write-Host "Done. Start BW2Color Movie from the Start menu or the desktop shortcut."
Write-Host "Uninstall: $dst\Uninstall.cmd"
