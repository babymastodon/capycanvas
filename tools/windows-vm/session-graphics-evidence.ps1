param(
    [Parameter(Mandatory)][string]$Output,
    [Parameter(Mandatory)][ValidateNotNullOrEmpty()][string]$ExpectedAccount,
    [Parameter(Mandatory)][ValidateRange(1,2147483647)][int]$ExpectedSessionId,
    [string]$ExpectedUserSid,
    [string]$ExpectedLogonSid,
    [string]$ExpectedBootUtc
)
$ErrorActionPreference='Stop'
$ProgressPreference='SilentlyContinue'
. (Join-Path $PSScriptRoot 'session-identity.ps1')
function Test-SessionGraphicsIdentity($Identity,$Expected) {
    $errors=@()
    $account=if($Expected.account.Contains('\')){$Identity.account}else{($Identity.account -split '\\')[-1]}
    if($account -ine $Expected.account){$errors+='Interactive account differs from the requested account'}
    if($Identity.session_id -le 0 -or $Identity.session_id -ne $Expected.session_id){$errors+='Interactive SessionId differs from the requested session'}
    if(!$Identity.user_sid){$errors+='Interactive token has no user SID'}
    if($Identity.logon_sid -notmatch '^S-1-5-5-\d+-\d+$'){$errors+='Interactive token has no unique logon SID'}
    if($Expected.user_sid -and $Identity.user_sid -cne $Expected.user_sid){$errors+='Interactive user SID differs from the requested SID'}
    if($Expected.logon_sid -and $Identity.logon_sid -cne $Expected.logon_sid){$errors+='Interactive logon SID differs from the requested logon'}
    $errors
}
function Read-SessionGraphicsProcesses([int]$SessionId) {
    $result=@{query_error=$null;rows=@()}
    try {
        $result.rows=@(Get-CimInstance Win32_Process -ErrorAction Stop|Where-Object {
            $_.SessionId -eq $SessionId -or $_.Name -match '^(CapyCanvas|capy-photo-reader|dwm|explorer)\.exe$'
        }|ForEach-Object {
            $native=$_;$row=$native|Select-Object Name,ProcessId,ParentProcessId,SessionId,CreationDate,HandleCount,ThreadCount,WorkingSetSize,PrivatePageCount,VirtualSize,ExecutablePath,CommandLine
            $owner=@{name=$null;domain=$null;sid=$null;return_value=$null;query_error=$null;sid_return_value=$null;sid_query_error=$null}
            try {$reply=Invoke-CimMethod -InputObject $native -MethodName GetOwner -ErrorAction Stop;$owner.return_value=$reply.ReturnValue
                if($reply.ReturnValue -eq 0){$owner.name=$reply.User;$owner.domain=$reply.Domain}else{$owner.query_error='GetOwner returned '+$reply.ReturnValue}
            } catch {$owner.query_error=$_.Exception.Message}
            try {$reply=Invoke-CimMethod -InputObject $native -MethodName GetOwnerSid -ErrorAction Stop;$owner.sid_return_value=$reply.ReturnValue
                if($reply.ReturnValue -eq 0){$owner.sid=$reply.Sid}else{$owner.sid_query_error='GetOwnerSid returned '+$reply.ReturnValue}
            } catch {$owner.sid_query_error=$_.Exception.Message}
            $memory=@{private_bytes=$null;process_start_utc=$null;session_id=$null;query_error=$null};$process=$null
            try {$process=Get-Process -Id $native.ProcessId -ErrorAction Stop;$memory.process_start_utc=$process.StartTime.ToUniversalTime().ToString('o');$memory.session_id=$process.SessionId
                if($process.SessionId -ne $native.SessionId){throw 'Process changed session during memory collection'}
                $memory.private_bytes=$process.PrivateMemorySize64
            } catch {$memory.query_error=$_.Exception.Message} finally {if($process){$process.Dispose()}}
            $row|Add-Member -NotePropertyName owner -NotePropertyValue $owner
            $row|Add-Member -NotePropertyName memory -NotePropertyValue $memory
            $row
        })
    } catch {$result.query_error=$_.Exception.Message}
    $result
}
function Read-SessionGraphicsEvents([datetime]$Boot) {
    $logs=[ordered]@{};$queries=@()
    foreach($pattern in 'Application','System','*Dwm*','*DesktopWindowManager*','*RdpCoreTS*','*DxgKrnl*') {
        $query=@{pattern=$pattern;query_error=$null;error_id=$null}
        try {foreach($log in @(Get-WinEvent -ListLog $pattern -ErrorAction Stop)) {
            $logs[$log.LogName]=$log|Select-Object LogName,IsEnabled,RecordCount,LogMode,LastWriteTime
        }} catch {$query.query_error=$_.Exception.Message;$query.error_id=$_.FullyQualifiedErrorId}
        $queries+=$query
    }
    $events=@(foreach($log in $logs.Values) {
        $row=@{log=$log.LogName;enabled=$log.IsEnabled;start_utc=$Boot.ToUniversalTime().ToString('o');status='disabled';query_error=$null;error_id=$null;scan_count=0;scan_capped=$false;events=@()}
        if($log.IsEnabled) {
            try {
                $batch=@(Get-WinEvent -FilterHashtable @{LogName=$log.LogName;StartTime=$Boot} -MaxEvents 4000 -ErrorAction Stop)
                $row.status='collected';$row.scan_count=$batch.Count;$row.scan_capped=$batch.Count -eq 4000
                $row.events=@($batch|Where-Object {
                    $log.LogName -notin 'Application','System' -or $_.ProviderName -match 'Dwm|Desktop.*Window.*Manager|RdpCoreTS|RemoteDesktopServices|Display|nvlddmkm|DxgKrnl|Application Popup|Application Error|Windows Error Reporting'
                }|ForEach-Object {
                    $entry=@{utc=$_.TimeCreated.ToUniversalTime().ToString('o');provider=$_.ProviderName;id=$_.Id;record_id=$_.RecordId;process_id=$_.ProcessId;thread_id=$_.ThreadId;level=$_.LevelDisplayName;message=$null;message_error=$null;xml=$null;xml_error=$null}
                    try {$entry.message=$_.Message} catch {$entry.message_error=$_.Exception.Message}
                    try {$entry.xml=$_.ToXml()} catch {$entry.xml_error=$_.Exception.Message}
                    $entry
                })
            } catch {
                $row.error_id=$_.FullyQualifiedErrorId
                if($_.FullyQualifiedErrorId -like 'NoMatchingEventsFound*'){$row.status='no-events'}else{$row.status='query-error';$row.query_error=$_.Exception.Message}
            }
        }
        $row
    })
    @{inventory_queries=$queries;logs=@($logs.Values);channels=$events}
}
$record=[ordered]@{started_utc=[DateTime]::UtcNow.ToString('o');completed_utc=$null;status='identity-error';query_error=$null;boot_utc=$null;
    expected=@{account=$ExpectedAccount;session_id=$ExpectedSessionId;user_sid=$ExpectedUserSid;logon_sid=$ExpectedLogonSid;boot_utc=$ExpectedBootUtc};
    identity=$null;identity_errors=@();expected_logon_supplied=[bool]$ExpectedLogonSid;same_logon_verified=$false;
    processes=$null;dwm_processes=$null;nvidia_smi=$null;gpu_process_memory=$null;graphics_events=$null;sessions=$null}
try {
    $record.identity=Get-CapySessionIdentity
    $record.identity_errors=@(Test-SessionGraphicsIdentity $record.identity $record.expected)
    if(!$record.identity_errors.Count) {
        $boot=[DateTimeOffset]::Parse($record.identity.boot_utc,[Globalization.CultureInfo]::InvariantCulture).UtcDateTime
        $record.boot_utc=$record.identity.boot_utc
        if($ExpectedBootUtc -and $boot.ToUniversalTime() -ne [DateTimeOffset]::Parse($ExpectedBootUtc,[Globalization.CultureInfo]::InvariantCulture).UtcDateTime){$record.identity_errors+= 'Guest boot differs from the requested boot'}
    }
    if($record.identity_errors.Count){$record.status='identity-mismatch'}else{
        $record.same_logon_verified=[bool]$ExpectedLogonSid;$record.status='collected'
        $record.sessions=@{exit_code=$null;query_error=$null;lines=@()}
        try {$record.sessions.lines=@(quser.exe 2>&1|ForEach-Object {[string]$_});$record.sessions.exit_code=$LASTEXITCODE} catch {$record.sessions.query_error=$_.Exception.Message}
        $record.processes=Read-SessionGraphicsProcesses $record.identity.session_id
        $record.dwm_processes=@($record.processes.rows|Where-Object {$_.SessionId -eq $record.identity.session_id -and $_.Name -ieq 'dwm.exe'})
        $record.nvidia_smi=@{available=$false;path=$null;exit_code=$null;query_error=$null;output=@()}
        try {
            $nvidia=Get-Command nvidia-smi.exe -ErrorAction Stop;$record.nvidia_smi.available=$true;$record.nvidia_smi.path=$nvidia.Source
            $record.nvidia_smi.output=@(& $nvidia.Source -q -d 'PIDS,MEMORY' 2>&1|ForEach-Object {[string]$_});$record.nvidia_smi.exit_code=$LASTEXITCODE
        } catch {$record.nvidia_smi.query_error=$_.Exception.Message}
        $record.gpu_process_memory=@{query_error=$null;instances=@()}
        try {$record.gpu_process_memory.instances=@(Get-CimInstance Win32_PerfFormattedData_GPUPerformanceCounters_GPUProcessMemory -ErrorAction Stop|Select-Object Name,DedicatedUsage,SharedUsage,TotalCommitted)}catch{$record.gpu_process_memory.query_error=$_.Exception.Message}
        $record.graphics_events=Read-SessionGraphicsEvents $boot
    }
} catch {$record.query_error=$_.Exception.Message}
$record.completed_utc=[DateTime]::UtcNow.ToString('o')
if(![IO.Path]::IsPathRooted($Output)){throw 'Session evidence output must be absolute'}
$directory=[IO.Path]::GetDirectoryName($Output);[IO.Directory]::CreateDirectory($directory)|Out-Null
$pending=$Output+'.'+$PID+'.pending'
[IO.File]::WriteAllText($pending,($record|ConvertTo-Json -Depth 10),[Text.UTF8Encoding]::new($false))
[IO.File]::Move($pending,$Output)
if($record.status -ne 'collected' -or $record.query_error){exit 1}
