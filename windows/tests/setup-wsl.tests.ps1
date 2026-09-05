param([switch] $Integration)
# Dependency-free workflow tests. Installation, console, and input calls are mocked.
$ErrorActionPreference = 'Stop'
. "$PSScriptRoot\..\setup\setup-wsl.ps1"
if ($Integration) {
    # Read-only regression for WSL's handling of quoted switches and spaced arguments.
    $env:WSL_UTF8 = '1'
    $code = Invoke-WslInteractive @('--exec', 'sh', '-c', 'exit 7')
    if ($code -ne 7) { throw "Actual WSL launcher returned $code instead of 7" }
    Write-Host 'PASS: actual WSL console launcher preserves arguments and exit status'
}
function Assert($Condition, [string] $Message) { if (-not $Condition) { throw $Message } }
function Invoke-Case([string] $Name, [scriptblock] $Test) {
    $script:responses = [Collections.Queue]::new()
    $script:answers = [Collections.Queue]::new()
    $script:interactive = [Collections.Queue]::new()
    $script:calls = [Collections.Generic.List[string]]::new()
    $script:elevations = 0
    $script:engineCode = 0
    & $Test
    Assert ($script:responses.Count -eq 0) "$Name left expected WSL calls unused"
    Assert ($script:interactive.Count -eq 0) "$Name left expected interactive calls unused"
    Write-Host "PASS: $Name"
}
function Expect([string] $Command, [int] $Code, [string] $Text = '') {
    $script:responses.Enqueue(@{ Command = $Command; Code = $Code; Text = $Text })
}
function Invoke-WslCapture([string[]] $Arguments) {
    $command = $Arguments -join '|'
    $script:calls.Add($command)
    Assert ($script:responses.Count -gt 0) "Unexpected WSL command: $command"
    $response = $script:responses.Dequeue()
    Assert ($response.Command -eq $command) "Expected $($response.Command), got $command"
    return $response
}
function Invoke-WslInteractive([string[]] $Arguments) {
    $command = $Arguments -join '|'
    Assert ($script:interactive.Count -gt 0) "Unexpected interactive WSL command: $command"
    $response = $script:interactive.Dequeue()
    Assert ($response.Command -eq $command) "Expected $($response.Command), got $command"
    return $response.Code
}
function Install-WslComponents { $script:elevations++; return $script:engineCode }
function Read-Host([string] $Prompt) {
    if ($script:answers.Count) { return $script:answers.Dequeue() }
    return ''
}
Invoke-Case 'Missing WSL installs components only and requests a restart' {
    Expect '--status' 1
    Assert ((Invoke-AitermWslSetup) -eq 3010) 'Expected restart'
    Assert ($script:elevations -eq 1) 'Expected one elevation'
}
Invoke-Case 'Declining component install makes no changes' {
    Expect '--status' 1
    $script:answers.Enqueue('n')
    Assert ((Invoke-AitermWslSetup) -eq 2) 'Expected cancellation'
    Assert ($script:elevations -eq 0) 'Must not elevate'
}
Invoke-Case 'Component failure does not install a distribution' {
    Expect '--status' 1
    $script:engineCode = 5
    $failed = $false
    try { Invoke-AitermWslSetup | Out-Null } catch { $failed = $_ -match 'exit 5' }
    Assert $failed 'Expected component error'
}
Invoke-Case 'Fresh Ubuntu setup verifies a regular account before selecting it' {
    Expect '--status' 0
    Expect '--list|--quiet' 0
    $script:interactive.Enqueue(@{ Command = '--install|--distribution|Ubuntu|--no-launch'; Code = 0 })
    $script:interactive.Enqueue(@{ Command = '--distribution|Ubuntu'; Code = 0 })
    Expect '--distribution|Ubuntu|--exec|id|-u' 0 '1000'
    Expect '--set-default|Ubuntu' 0
    Expect '--distribution|Ubuntu|--exec|wslinfo|--networking-mode' 0 'mirrored'
    Assert ((Invoke-AitermWslSetup) -eq 0) 'Expected ready'
    Assert ($script:elevations -eq 0) 'Account setup must not elevate'
}
Invoke-Case 'Existing distro selection excludes Docker and preserves other distributions' {
    Expect '--status' 0
    Expect '--list|--quiet' 0 "docker-desktop`r`nDebian`r`nUbuntu Custom"
    $script:answers.Enqueue('2')
    $script:interactive.Enqueue(@{ Command = '--distribution|Ubuntu Custom'; Code = 0 })
    Expect '--distribution|Ubuntu Custom|--exec|id|-u' 0 '1001'
    Expect '--set-default|Ubuntu Custom' 0
    Expect '--distribution|Ubuntu Custom|--exec|wslinfo|--networking-mode' 0 'nat'
    Assert ((Invoke-AitermWslSetup) -eq 0) 'Expected ready'
    Assert ($script:elevations -eq 0) 'Must not elevate existing distro'
}
Invoke-Case 'Root account cannot complete setup' {
    Expect '--status' 0
    Expect '--list|--quiet' 0 'Ubuntu'
    $script:interactive.Enqueue(@{ Command = '--distribution|Ubuntu'; Code = 0 })
    Expect '--distribution|Ubuntu|--exec|id|-u' 0 '0'
    $failed = $false
    try { Invoke-AitermWslSetup | Out-Null } catch { $failed = $_ -match 'regular Linux user' }
    Assert $failed 'Expected account setup error'
}
Invoke-Case 'Distro installation restart stops before account creation' {
    Expect '--status' 0
    Expect '--list|--quiet' 0
    $script:interactive.Enqueue(@{ Command = '--install|--distribution|Ubuntu|--no-launch'; Code = 3010 })
    Assert ((Invoke-AitermWslSetup) -eq 3010) 'Expected restart'
}
Invoke-Case 'Distro installation failure remains retryable' {
    Expect '--status' 0
    Expect '--list|--quiet' 0
    $script:interactive.Enqueue(@{ Command = '--install|--distribution|Ubuntu|--no-launch'; Code = 1 })
    $failed = $false
    try { Invoke-AitermWslSetup | Out-Null } catch { $failed = $_ -match 'installation did not finish' }
    Assert $failed 'Expected installation error'
}
Invoke-Case 'Declining default change leaves the default unchanged' {
    Expect '--status' 0
    Expect '--list|--quiet' 0 'Debian'
    $script:answers.Enqueue('1')
    $script:answers.Enqueue('n')
    $script:interactive.Enqueue(@{ Command = '--distribution|Debian'; Code = 0 })
    Expect '--distribution|Debian|--exec|id|-u' 0 '1000'
    Assert ((Invoke-AitermWslSetup) -eq 2) 'Expected cancellation'
}
