$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

if (!(Test-Path -LiteralPath (Join-Path $env:ProgramFiles 'PowerShell\7\pwsh.exe'))) {
    $url = 'https://github.com/PowerShell/PowerShell/releases/download/v7.6.10/PowerShell-7.6.10-win-x64.msi'
    $expected = '7d8daea48b450d4d62fcd1ac4d6faa7ac2db5e11aba5061ee81b7496adca8c07'
    $installer = Join-Path $env:TEMP 'PowerShell-7.6.10-win-x64.msi'
    Invoke-WebRequest $url -OutFile $installer -UseBasicParsing
    $actual = (Get-FileHash -LiteralPath $installer -Algorithm SHA256).Hash
    if ($actual -ne $expected) { throw "PowerShell 7 package $url SHA256 mismatch: expected $expected; actual $actual." }
    $process = [Diagnostics.Process]::Start([Diagnostics.ProcessStartInfo]@{ FileName = 'msiexec.exe'; Arguments = "/i `"$installer`" /quiet /norestart ADD_PATH=1"; UseShellExecute = $false })
    $process.WaitForExit()
    Remove-Item -LiteralPath $installer
    if ($process.ExitCode -notin 0, 3010) { throw "PowerShell 7 setup exited with $($process.ExitCode)." }
}

function Enable-FixtureJapaneseInput {
    $capability = 'Language.Basic~~~ja-JP~0.0.1.0'
    if ((Get-WindowsCapability -Online -Name $capability).State -ne 'Installed') {
        $interactiveUser = [Environment]::UserName
        $deadline = (Get-Date).AddSeconds(120)
        while (!(Get-Process explorer -IncludeUserName -ErrorAction SilentlyContinue | Where-Object UserName -Like "*\$interactiveUser")) {
            if ((Get-Date) -ge $deadline) { throw 'The private user interactive desktop did not start' }
            Start-Sleep -Seconds 1
        }
        $taskName = 'capycanvas-japanese-input-' + [guid]::NewGuid().ToString('N')
        $command = "`$ErrorActionPreference='Stop';`$ProgressPreference='SilentlyContinue';Add-WindowsCapability -Online -Name '$capability' | Out-Null"
        $encoded = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($command))
        $action = New-ScheduledTaskAction -Execute (Get-Process -Id $PID).Path -Argument "-NoProfile -NonInteractive -WindowStyle Hidden -EncodedCommand $encoded"
        $principal = New-ScheduledTaskPrincipal -UserId $interactiveUser -LogonType Interactive -RunLevel Highest
        $settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit 0
        try {
            Register-ScheduledTask -TaskName $taskName -Action $action -Principal $principal -Settings $settings | Out-Null
            $previousRun = (Get-ScheduledTaskInfo -TaskName $taskName).LastRunTime
            Start-ScheduledTask -TaskName $taskName
            do {
                $info = Get-ScheduledTaskInfo -TaskName $taskName
                $state = (Get-ScheduledTask -TaskName $taskName -ErrorAction SilentlyContinue).State
                if ($info.LastRunTime -gt $previousRun -and $state -eq 'Ready' -and $info.LastTaskResult -ne 267009) { $result = $info.LastTaskResult; break }
                Start-Sleep -Seconds 1
            } while ($true)
            if ($result -ne 0) { throw "Japanese Basic input task $taskName failed: $result" }
        } finally {
            $task = Get-ScheduledTask -TaskName $taskName -ErrorAction SilentlyContinue
            if ($task -and $task.State -notin @('Running','Queued')) { Unregister-ScheduledTask -TaskName $taskName -Confirm:$false }
        }
        if ((Get-WindowsCapability -Online -Name $capability).State -ne 'Installed') {
            throw 'Japanese Basic input capability did not install'
        }
    }
}
Enable-FixtureJapaneseInput

$desktop = 'HKCU:\Control Panel\Desktop'
$personalize = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize'
Set-ItemProperty $personalize AppsUseLightTheme 0 -Type DWord
Set-ItemProperty $personalize SystemUsesLightTheme 0 -Type DWord
foreach ($monitor in Get-ChildItem 'HKLM:\SYSTEM\CurrentControlSet\Control\GraphicsDrivers\Configuration') {
    $settings = New-Item -Force (Join-Path $desktop "PerMonitorSettings\$($monitor.PSChildName)")
    Set-ItemProperty $settings.PSPath DpiValue 2 -Type DWord
}
foreach ($name in 'Configuration', 'Connectivity', 'ScaleFactors') {
    $displays = "HKLM:\SYSTEM\CurrentControlSet\Control\GraphicsDrivers\$name"
    if (Test-Path $displays) { Get-ChildItem $displays | Remove-Item -Recurse -Force }
}
