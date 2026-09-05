$ErrorActionPreference = 'Stop'
$dir = Join-Path $env:TEMP 'aiterm-installer-setup-check'
New-Item -ItemType Directory -Force $dir | Out-Null
$source = Get-Content "$PSScriptRoot\..\installer-hooks.nsh" -Raw
foreach ($case in @('actual', 'missing', 'root')) {
    $hook = $source
    if ($case -ne 'actual') {
        $uid = if ($case -eq 'root') { '0' } else { '' }
        $status = if ($case -eq 'root') { '0' } else { 'error' }
        $replacement = '  Push "' + $uid + '"' + "`r`n" + '  Push "' + $status + '"'
        $hook = [regex]::Replace($hook, '(?m)^  nsExec::ExecToStack[^\r\n]+--exec sh[^\r\n]+', $replacement)
    }
    # Silence networking checks here; that behavior has its own previous harness.
    $hook = $hook.Replace('Call AITermCheckWslNetworking', 'DetailPrint "Workspace ready"')
    # If silent mode ever launches setup, fail rather than installing anything.
    $hook = [regex]::Replace($hook, '(?m)^    Exec [^\r\n]+', '    Abort "Silent installation must never launch WSL setup"')
    Set-Content "$dir\$case.nsh" $hook
    $nsi = @'
Unicode true
Name "AITerm WSL setup check"
OutFile "CASE.exe"
RequestExecutionLevel user
SilentInstall silent
!include "CASE.nsh"
Section
  !insertmacro NSIS_HOOK_PREINSTALL
  !insertmacro NSIS_HOOK_POSTINSTALL
  FileOpen $9 "RESULT" w
  FileWrite $9 "$AITermNeedsWslSetup"
  FileClose $9
SectionEnd
'@.Replace('CASE', $case).Replace('RESULT', "$dir\$case.txt")
    Set-Content "$dir\$case.nsi" $nsi
    & "$env:LOCALAPPDATA\tauri\NSIS\makensis.exe" /V2 "$dir\$case.nsi"
    if ($LASTEXITCODE -ne 0) { throw "NSIS compilation failed: $case" }
    $process = Start-Process "$dir\$case.exe" -ArgumentList '/S' -PassThru
    if (-not $process.WaitForExit(30000)) { $process.Kill(); throw "Installer hung: $case" }
    if ($process.ExitCode -ne 0) { throw "Installer failed: $case" }
    $result = Get-Content "$dir\$case.txt" -Raw
    $expected = if ($case -eq 'actual') { '0' } else { '1' }
    if ($result -ne $expected) { throw "Expected setup flag $expected, got $result for $case" }
    Write-Output "PASS: $case (silent setup skipped, needs-setup=$result)"
}
