$ErrorActionPreference='Stop'
$tokens=$null;$errors=$null
$collector=[Management.Automation.Language.Parser]::ParseFile((Join-Path $PSScriptRoot 'session-graphics-evidence.ps1'),[ref]$tokens,[ref]$errors)
if(@($errors).Count){throw ($errors|Out-String)}
$functions=@($collector.EndBlock.Statements|Where-Object {$_ -is [Management.Automation.Language.FunctionDefinitionAst]})
function Import-SessionGraphicsFunctions {
    foreach($definition in $functions){. ([scriptblock]::Create($definition.Extent.Text))}
}
& {
    . Import-SessionGraphicsFunctions
    $expected=@{account='capy14';session_id=15;user_sid='S-1-5-21-14';logon_sid='S-1-5-5-1-14'}
    $identity=@{account='VM\capy14';session_id=15;user_sid='S-1-5-21-14';logon_sid='S-1-5-5-1-14'}
    if(@(Test-SessionGraphicsIdentity $identity $expected).Count){throw 'Matching interactive identity was rejected'}
    foreach($fault in 'account','session','user','logon','missing-logon'){
        $changed=$identity.Clone()
        switch($fault){
            'account'{$changed.account='VM\capy13'}
            'session'{$changed.session_id=0}
            'user'{$changed.user_sid='S-1-5-21-13'}
            'logon'{$changed.logon_sid='S-1-5-5-2-14'}
            'missing-logon'{$changed.logon_sid=$null}
        }
        if(!@(Test-SessionGraphicsIdentity $changed $expected).Count){throw "Invalid interactive identity was accepted: $fault"}
    }
}
& {
    . Import-SessionGraphicsFunctions
    function Get-CimInstance {[CmdletBinding()]param($ClassName)
        [pscustomobject]@{Name='CapyCanvas.exe';ProcessId=41;SessionId=15;PrivatePageCount=123}
    }
    function Invoke-CimMethod {[CmdletBinding()]param($InputObject,$MethodName)
        if($MethodName -eq 'GetOwner'){[pscustomobject]@{ReturnValue=2}}else{throw 'owner SID denied'}
    }
    function Get-Process {[CmdletBinding()]param($Id)
        $value=[pscustomobject]@{StartTime=[datetime]'2026-11-11T09:00:00Z';SessionId=15;PrivateMemorySize64=4096}
        $value|Add-Member ScriptMethod Dispose {}; $value
    }
    $result=Read-SessionGraphicsProcesses 15
    if($null -ne $result.query_error -or $result.rows.Count -ne 1){throw 'Process evidence query failed or lost its row'}
    $row=$result.rows[0]
    if($row.owner.return_value -ne 2 -or $null -ne $row.owner.name -or $row.owner.sid_query_error -notlike '*denied*'){throw 'Owner denial became a known owner or lost its error'}
    if($row.memory.private_bytes -ne 4096){throw 'Process private bytes were replaced with page count or zero'}
}
& {
    . Import-SessionGraphicsFunctions
    function Get-WinEvent {[CmdletBinding()]param($ListLog,$FilterHashtable,$MaxEvents)
        if($ListLog){
            switch($ListLog){
                'Application'{[pscustomobject]@{LogName='Application';IsEnabled=$true}}
                'System'{[pscustomobject]@{LogName='System';IsEnabled=$true}}
                '*Dwm*'{[pscustomobject]@{LogName='Dwm-Core/Operational';IsEnabled=$false}}
            }
        }else{
            if($FilterHashtable.LogName -eq 'Dwm-Core/Operational'){throw 'Disabled channel was read'}
            if($FilterHashtable.LogName -eq 'System'){throw 'System access denied'}
            $entry=[pscustomobject]@{TimeCreated=[datetime]'2026-11-11T09:00:00Z';ProviderName='Application Popup';Id=26;RecordId=17;ProcessId=41;ThreadId=2;LevelDisplayName='Error';Message='graphics'}
            $entry|Add-Member ScriptMethod ToXml {'<Event/>'};$entry
        }
    }
    $result=Read-SessionGraphicsEvents ([datetime]'2026-11-11T06:00:00Z')
    $channels=@{};foreach($row in $result.channels){$channels[$row.log]=$row}
    if($channels['Dwm-Core/Operational'].status -cne 'disabled'){throw 'Disabled event channel was queried'}
    if($channels.System.status -cne 'query-error' -or $channels.System.query_error -notlike '*denied*'){throw 'Denied event query became no-events'}
    if($channels.Application.events.Count -ne 1 -or $channels.Application.events[0].id -ne 26){throw 'Graphics event was not retained'}
}
& {
    $directory=Join-Path ([IO.Path]::GetTempPath()) ('capy-session-graphics-'+[guid]::NewGuid().ToString('N'))
    [IO.Directory]::CreateDirectory($directory)|Out-Null
    $priorExit=$global:LASTEXITCODE
    try{
        $child=Join-Path $directory 'wrong-session.ps1';$output=Join-Path $directory 'evidence.json'
        $body=@($collector.EndBlock.Statements|Where-Object {$_ -isnot [Management.Automation.Language.FunctionDefinitionAst] -and $_.Extent.Text -notlike '. (Join-Path *session-identity.ps1*)'})
        $source='param([string]$Output)'+"`n"+($functions.Extent.Text -join "`n")+@'

function Get-CapySessionIdentity {@{process_id=41;session_id=0;account='VM\capy14';user_sid='S-1-5-21-14';logon_sid='S-1-5-5-1-14'}}
function Get-CimInstance {throw 'Wrong session queried resources'}
$ExpectedAccount='capy14';$ExpectedSessionId=15;$ExpectedUserSid='';$ExpectedLogonSid='';$ExpectedBootUtc=''
'@+"`n"+($body.Extent.Text -join "`n")
        [IO.File]::WriteAllText($child,$source,[Text.UTF8Encoding]::new($false))
        $shell=(Get-Process -Id $PID).Path
        & $shell -NoProfile -NonInteractive -File $child -Output $output
        if($LASTEXITCODE -ne 1){throw 'Wrong-session collector did not exit with its predicted failure'}
        $record=[IO.File]::ReadAllText($output)|ConvertFrom-Json
        if($record.status -cne 'identity-mismatch' -or $null -ne $record.query_error){throw 'Wrong-session receipt lost its identity failure'}
        if($null -ne $record.processes -or $null -ne $record.graphics_events -or $record.same_logon_verified){throw 'Wrong-session collector queried resources or claimed matching logon'}
    }finally{$global:LASTEXITCODE=$priorExit;Remove-Item -LiteralPath $directory -Recurse -Force}
}
if([Environment]::OSVersion.Platform -eq [PlatformID]::Win32NT){
    & {
        . (Join-Path $PSScriptRoot 'session-identity.ps1')
        $actual=Get-CapySessionIdentity
        $process=[Diagnostics.Process]::GetCurrentProcess();$token=[Security.Principal.WindowsIdentity]::GetCurrent()
        try{
            if($actual.session_id -le 0 -or $actual.session_id -ne $process.SessionId){throw 'Actual identity is not the interactive process session'}
            if($actual.account -cne $token.Name -or $actual.user_sid -cne $token.User.Value){throw 'Actual identity lost the current token account or SID'}
            if($actual.logon_sid -notmatch '^S-1-5-5-\d+-\d+$'){throw 'Actual token has no logon SID'}
            if($actual.process_id -ne $PID -or $actual.process_start_utc -cne $process.StartTime.ToUniversalTime().ToString('o')){throw 'Actual identity lost the current PID or birth'}
            if($actual.boot_utc -cne (Get-CimInstance Win32_OperatingSystem -ErrorAction Stop).LastBootUpTime.ToUniversalTime().ToString('o')){throw 'Actual identity lost the current guest boot'}
        }finally{$token.Dispose();$process.Dispose()}
    }
}
Write-Output 'PASS session graphics evidence'
