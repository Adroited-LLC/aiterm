param([string]$Distribution = 'Ubuntu', [switch]$Bundle)
$ErrorActionPreference = 'Stop'
$repo = Split-Path $PSScriptRoot -Parent
$env:Path = [Environment]::GetEnvironmentVariable('Path','Machine') + ';' + [Environment]::GetEnvironmentVariable('Path','User')
$env:WSL_UTF8 = '1'

function Check-Exit([string]$Step) {
    if ($LASTEXITCODE -ne 0) { throw "$Step failed with exit code $LASTEXITCODE" }
}

Push-Location $repo
try {
    Write-Host 'Building the Linux companion in WSL...'
    $linuxRepo = (& wsl.exe --distribution $Distribution --exec wslpath -u $repo).Trim()
    Check-Exit 'WSL path conversion'
    & wsl.exe --distribution $Distribution --exec sh "$linuxRepo/scripts/build-wsl-companion.sh" $linuxRepo
    Check-Exit 'Linux build'
    & npm.cmd ci
    Check-Exit 'Frontend dependencies'
    & npm.cmd run build:windows-ui
    Check-Exit 'Windows frontend'
    Push-Location (Join-Path $repo 'windows')
    try {
        if ($Bundle) { & "$repo\node_modules\.bin\tauri.cmd" build -- --locked }
        else { & "$repo\node_modules\.bin\tauri.cmd" build --no-bundle -- --locked }
        Check-Exit 'Windows desktop'
    } finally { Pop-Location }
    $env:AITERM_SMOKE_REPORT = Join-Path $repo 'windows\target\smoke-test.txt'
    $test = Start-Process -FilePath "$repo\windows\target\release\aiterm-windows.exe" -ArgumentList '--smoke-test' -PassThru
    if (!$test.WaitForExit(120000)) { $test.Kill(); throw 'Windows/WSL smoke test timed out' }
    if ($test.ExitCode -ne 0) { throw (Get-Content $env:AITERM_SMOKE_REPORT -Raw) }
    Get-Content $env:AITERM_SMOKE_REPORT
} finally { Pop-Location }
