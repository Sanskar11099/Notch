# Build the notch from the source next to this script and install it on drive C, so it keeps working with E disconnected.
# Run from anywhere:  powershell -File "E:\Project Notch\deploy.ps1"
$ErrorActionPreference = 'Stop'
$src     = $PSScriptRoot
$install = Join-Path $env:LOCALAPPDATA 'Notch'
$exe     = Join-Path $install 'notch.exe'
$wrapper = Join-Path $env:USERPROFILE '.notch\notch.cmd'
$env:Path += ";$env:USERPROFILE\.cargo\bin"

Get-Process notch -ErrorAction SilentlyContinue | Stop-Process -Force # a running notch locks its exe, which would fail the build
Start-Sleep -Milliseconds 400
Push-Location $src
try { cargo test --quiet; if ($LASTEXITCODE) { throw 'tests failed, nothing installed' }
      cargo build --release --quiet; if ($LASTEXITCODE) { throw 'build failed, nothing installed' } }
finally { Pop-Location }

New-Item -ItemType Directory -Force $install | Out-Null
Copy-Item "$src\target\release\notch.exe" $exe -Force
# The Chrome extension: Chrome loads it from this folder, so it stays on C too (reload it in chrome://extensions after a deploy).
Copy-Item "$src\extension" $install -Recurse -Force -Exclude '*.test.js'

# Everything that launches the notch points at the C copy: the agent-hook wrapper and start-at-login.
New-Item -ItemType Directory -Force (Split-Path $wrapper) | Out-Null
Set-Content $wrapper "@echo off`r`nrem Space-free entry point for agent hooks. Written by deploy.ps1.`r`n`"$exe`" %*" -Encoding ascii
Set-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run' -Name Notch -Value "`"$exe`""

Start-Process $exe
Write-Host "Installed $exe ($([math]::Round((Get-Item $exe).Length/1MB,1)) MB) and started it. Extension copy: $install\extension"
