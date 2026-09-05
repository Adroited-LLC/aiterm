# Run as the signed-in Windows user. Only Windows component installation elevates.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Invoke-WslCapture([string[]] $Arguments) {
    $oldPreference = $ErrorActionPreference
    try {
        $ErrorActionPreference = 'Continue'
        $output = & "$env:SystemRoot\System32\wsl.exe" @Arguments 2>&1
        $code = $LASTEXITCODE
        return @{ Code = $code; Text = (($output | Out-String) -replace "`0", '').Trim() }
    } finally { $ErrorActionPreference = $oldPreference }
}

function Install-WslComponents {
    # No distribution is registered under an elevated or alternate admin account.
    $command = '& "$env:SystemRoot\System32\wsl.exe" --install --no-distribution; exit $LASTEXITCODE'
    $encoded = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($command))
    $process = Start-Process -FilePath "$env:SystemRoot\System32\WindowsPowerShell\v1.0\powershell.exe" -Verb RunAs -Wait -PassThru -ArgumentList @('-NoProfile', '-EncodedCommand', $encoded)
    return $process.ExitCode
}

function Invoke-WslInteractive([string[]] $Arguments) {
    # Start-Process inherits the console, including unredirected password input.
    $quoted = @($Arguments | ForEach-Object {
        # WSL's option parser expects bare switches, not quoted "--exec".
        if ($_ -and $_ -notmatch '[\s"]') { $_ }
        else { '"' + [regex]::Replace([regex]::Replace($_, '(\\*)"', '$1$1\"'), '(\\+)$', '$1$1') + '"' }
    })
    $process = Start-Process -FilePath "$env:SystemRoot\System32\wsl.exe" -ArgumentList $quoted -NoNewWindow -Wait -PassThru
    return $process.ExitCode
}

function Invoke-AitermWslSetup {
    Write-Host "`nWelcome to AITerm" -ForegroundColor Cyan
    Write-Host 'We will help you prepare the Linux workspace that AITerm uses.'
    Write-Host 'You will need an internet connection. Downloads can take several minutes.'
    $env:WSL_UTF8 = '1'
    $status = Invoke-WslCapture @('--status')
    if ($status.Code -ne 0) {
        Write-Host "`nWindows needs WSL enabled or repaired. Windows will ask for administrator approval."
        $answer = Read-Host 'Install WSL components now? [Y/n]'
        if ($answer -and $answer -notmatch '^(y|yes)$') { return 2 }
        $code = Install-WslComponents
        if ($code -ne 0 -and $code -ne 3010) {
            throw "Windows could not finish installing WSL (exit $code). Check that virtualization is enabled. In a virtual machine, nested virtualization must be available."
        }
        Write-Host "`nSave your work and restart Windows, then open AITerm and choose Set up WSL again." -ForegroundColor Yellow
        Write-Host 'The next step installs Ubuntu and creates your Linux account. We will not restart your PC automatically.'
        Read-Host 'Press Enter to close this window' | Out-Null
        return 3010
    }

    $listed = Invoke-WslCapture @('--list', '--quiet')
    if ($listed.Code -ne 0) { throw "Could not list Linux distributions. $($listed.Text)" }
    $distributions = @($listed.Text -split '\r?\n' | ForEach-Object { $_.Trim() } | Where-Object { $_ -and $_ -notlike 'docker-desktop*' })
    if ($distributions.Count -eq 0) {
        Write-Host "`nAITerm needs a Linux distribution. Ubuntu is the default choice."
        $answer = Read-Host 'Download and install Ubuntu now? [Y/n]'
        if ($answer -and $answer -notmatch '^(y|yes)$') { return 2 }
        $code = Invoke-WslInteractive @('--install', '--distribution', 'Ubuntu', '--no-launch')
        if ($code -eq 3010) {
            Write-Host 'Save your work, restart Windows, then open AITerm to continue setup.'
            Read-Host 'Press Enter to close this window' | Out-Null
            return 3010
        }
        if ($code -ne 0) { throw 'Ubuntu installation did not finish. Follow the Windows message above, then run setup again.' }
        $distribution = 'Ubuntu'
    } else {
        Write-Host "`nUse an existing Linux distribution for AITerm:"
        for ($i = 0; $i -lt $distributions.Count; $i++) { Write-Host "  $($i + 1). $($distributions[$i])" }
        $choice = Read-Host 'Choose a number (Enter for 1)'
        if (-not $choice) { $choice = '1' }
        $index = 0
        if (-not [int]::TryParse($choice, [ref]$index) -or $index -lt 1 -or $index -gt $distributions.Count) { throw 'Please run setup again and choose a number from the list.' }
        $distribution = $distributions[$index - 1]
    }

    Write-Host "`nOpening $distribution. If this is your first launch, follow its account setup prompts."
    Write-Host 'Choose a Linux username and password. The password is separate from your Windows password.'
    Write-Host 'Password characters may not appear as you type. AITerm does not collect or save this password.'
    Write-Host 'When you see your Linux command prompt, type exit and press Enter to return here.'
    $code = Invoke-WslInteractive @('--distribution', $distribution)
    if ($code -ne 0) { throw 'Linux setup was interrupted. Run setup again to continue.' }
    $account = Invoke-WslCapture @('--distribution', $distribution, '--exec', 'id', '-u')
    if ($account.Code -ne 0 -or $account.Text -notmatch '^\d+$' -or $account.Text -eq '0') {
        throw 'A regular Linux user is not ready yet. Complete your distribution account setup and make that user its default, then try again.'
    }
    $answer = Read-Host "Use $distribution as the default Linux distribution for AITerm and Windows WSL commands? [Y/n]"
    if ($answer -and $answer -notmatch '^(y|yes)$') { return 2 }
    $selected = Invoke-WslCapture @('--set-default', $distribution)
    if ($selected.Code -ne 0) { throw "Could not select the default distribution. $($selected.Text)" }
    Write-Host "`nYour Linux account is ready." -ForegroundColor Green
    $network = Invoke-WslCapture @('--distribution', $distribution, '--exec', 'wslinfo', '--networking-mode')
    if ($network.Code -ne 0 -or $network.Text -ne 'mirrored') {
        Write-Host 'For direct phone connections, open WSL Settings > Networking and choose Mirrored (Windows 11 22H2 or later).'
        Write-Host 'Save your Linux work, then restart WSL or Windows to apply it. Your firewall must allow AITerm.'
    }
    Read-Host 'Press Enter to return to AITerm' | Out-Null
    return 0
}

# Dot-sourcing exposes the workflow for tests without installing anything.
if ($MyInvocation.InvocationName -ne '.') {
    $mutex = [Threading.Mutex]::new($false, 'Local\AITermWslSetup')
    $owned = $false
    try {
        try { $owned = $mutex.WaitOne(0) } catch [Threading.AbandonedMutexException] { $owned = $true }
        if (-not $owned) {
            Write-Host 'AITerm setup is already open. Complete the existing setup window.'
            Read-Host 'Press Enter to close this window' | Out-Null
            exit 2
        }
        exit (Invoke-AitermWslSetup)
    }
    catch {
        Write-Host "`nSetup could not finish: $_" -ForegroundColor Yellow
        Write-Host 'You can close this window and choose Set up WSL in AITerm to retry.'
        Write-Host 'Help: https://learn.microsoft.com/windows/wsl/install'
        Read-Host 'Press Enter to close this window' | Out-Null
        exit 1
    }
    finally {
        if ($owned) { $mutex.ReleaseMutex() }
        $mutex.Dispose()
    }
}
