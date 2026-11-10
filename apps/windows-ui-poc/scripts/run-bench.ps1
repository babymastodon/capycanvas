param(
    [Parameter(Mandatory)][ValidateSet('capycanvas6-poc-gpu','capycanvas6-poc-warp')][string]$Vm,
    [Parameter(Mandatory)][string]$Out,
    [Parameter(Mandatory)][string]$PresentMon,
    [Parameter(Mandatory)][string]$PresentMonSha256,
    [ValidateSet('normal','lowcpu')][string]$Profile='normal',
    [Alias('Arm')][ValidateSet('abba','retained','winui')][string]$ArmOrder='abba',
    [Alias('Workload')][string[]]$WorkloadNames,
    [string]$TargetDirectory
)
$ErrorActionPreference='Stop'
$root=(Resolve-Path (Join-Path $PSScriptRoot '..')).Path
[IO.Directory]::CreateDirectory($Out)|Out-Null
if(!$env:CAPY_PRIVATE_DESKTOP){
    & (Join-Path $root '../layer-windows/scripts/test-desktop.ps1') -Script $PSCommandPath -Parameters $PSBoundParameters -TimeoutSeconds 43200
    return
}
if(!$TargetDirectory){$TargetDirectory=Join-Path $root 'target'}
$bin=Join-Path $TargetDirectory 'release'
$observer=Join-Path $bin 'poc-observer.exe'
$build=Get-Content -LiteralPath (Join-Path $bin 'build.json') -Raw|ConvertFrom-Json
foreach($file in $build.files){if((Get-FileHash -LiteralPath (Join-Path $bin $file.path) -Algorithm SHA256).Hash -ine $file.sha256){throw "Build hash mismatch: $($file.path)"}}
if((Get-FileHash -LiteralPath $PresentMon -Algorithm SHA256).Hash -ine $PresentMonSha256){throw 'PresentMon SHA256 mismatch'}
$workloads=@('startup','paint-still','divider','resize-programmatic','resize-native','scroll-wheel','scroll-touch','drawer','zen','scrub-while-painting','typing','stall-drawer','stall-scroll','slowcanvas-divider','slowcanvas-drawer')
if($WorkloadNames){foreach($name in $WorkloadNames){if($name -notin $workloads){throw "Unknown workload $name"}};$workloads=$WorkloadNames}
$arms=@(if($ArmOrder -eq 'abba'){'retained','winui','winui','retained'}else{$ArmOrder})
$records=[Collections.Generic.List[object]]::new()
$overall=$false
try{
    foreach($workload in $workloads){
        for($slot=0;$slot -lt $arms.Count;$slot++){
            $arm=$arms[$slot]
            $runtimeOnly=$Vm -eq 'capycanvas6-poc-warp' -or $arm -eq 'winui'
            $presentmonMode=if($runtimeOnly){'runtime-only'}else{'full'}
            $runtimeReason=if($Vm -eq 'capycanvas6-poc-warp'){'Approved WARP-only exception: PresentMode and display timing are unverified; PresentMon records runtime events only'}elseif($runtimeOnly){'Approved GPU WinUI-only exception: PresentMode and display timing are unverified; PresentMon records runtime events only'}else{$null}
            $executable=Join-Path $bin (@{retained='poc-retained.exe';winui='PocWinUI.exe'}[$arm])
            $captureModes=if($slot -lt 2 -and $workload -in 'paint-still','divider'){@($false,$true)}else{@($false)}
            foreach($noCapture in $captureModes){
                $folder=Join-Path $Out ("$Profile/$workload/$slot-$arm-"+$(if($noCapture){'nocapture'}else{'capture'}))
                [IO.Directory]::CreateDirectory($folder)|Out-Null
                $session='CapyPoc-'+[Guid]::NewGuid().ToString('N')
                $csv=Join-Path $folder 'presents.csv'
                $monitor=$null
                $code=$null
                $collectorCode=$null
                try{
                    $monitorArguments=@('--process_name',([IO.Path]::GetFileName($executable)),'--session_name',$session,'--v1_metrics','--qpc_time','--no_console_stats','--output_file',('"'+$csv+'"'))
                    if($runtimeOnly){$monitorArguments+=@('--no_track_display','--no_track_gpu','--no_track_input')}
                    $monitor=Start-Process -FilePath $PresentMon -ArgumentList $monitorArguments -PassThru -WindowStyle Hidden -RedirectStandardOutput (Join-Path $folder 'presentmon.log') -RedirectStandardError (Join-Path $folder 'presentmon-error.log')
                    $null=$monitor.Handle
                    Start-Sleep -Milliseconds 500
                    if($monitor.HasExited){throw "PresentMon exited before observation: $($monitor.ExitCode)"}
                    $arguments=@('run','--arm',$arm,'--exe',$executable,'--workload',$workload,'--contacts','3','--seconds','6','--warmup','1','--profile',$Profile,'--out',$folder)
                    if($noCapture){$arguments+='--no-capture'}
                    $arguments+='--'
                    $arguments+=@('--thumbnails',$(if($workload -in 'scroll-wheel','scroll-touch','stall-scroll'){'on'}else{'off'}),'--canvas-load',$(if($workload.StartsWith('slowcanvas-')){'slow'}else{'light'}))
                    if($workload.StartsWith('stall-')){$arguments+=@('--ui-stall-ms','20','--ui-stall-every-ms','100')}
                    & $observer @arguments 1>(Join-Path $folder 'observer.log') 2>(Join-Path $folder 'observer-error.log')
                    $code=$LASTEXITCODE
                    if($code){throw "Observer $arm/$workload exited $code; see $folder"}
                }finally{
                    if($monitor -and !$monitor.HasExited){
                        $previous=$ErrorActionPreference
                        try{
                            $ErrorActionPreference='Continue'
                            & $PresentMon --session_name $session --terminate_existing_session 1>(Join-Path $folder 'presentmon-stop.log') 2>(Join-Path $folder 'presentmon-stop-error.log')
                            $stopCode=$LASTEXITCODE
                        }finally{$ErrorActionPreference=$previous}
                        if($stopCode){throw "Owned PresentMon stop exited $stopCode"}
                        if(!$monitor.WaitForExit(15000)){$monitor.Kill();throw 'Owned PresentMon did not flush after stopping its session'}
                    }
                    if($monitor -and $monitor.HasExited){$collectorCode=$monitor.ExitCode}
                    $csvHasRows=(Test-Path -LiteralPath $csv) -and (@(Get-Content -LiteralPath $csv -TotalCount 2).Count -gt 1)
                    $contacts=@(foreach($metaFile in Get-ChildItem -LiteralPath $folder -Filter meta.json -Recurse -File){
                        $meta=Get-Content -LiteralPath $metaFile.FullName -Raw|ConvertFrom-Json
                        $canvasTrace=Join-Path $metaFile.DirectoryName 'canvas.jsonl'
                        $canvasPresents=0;$uiPresents=0;$adapter=$null
                        if(Test-Path -LiteralPath $canvasTrace){foreach($line in Get-Content -LiteralPath $canvasTrace -Encoding UTF8){
                            if(!$line){continue};$entry=$line|ConvertFrom-Json
                            switch($entry.kind){'canvas_frame'{$canvasPresents++};'ui_present'{$uiPresents++};'adapter'{if(!$adapter){$adapter=$entry}}}
                        }}
                        if($adapter){$meta|Add-Member -NotePropertyName adapter -NotePropertyValue $adapter -Force}
                        @{file=$metaFile;meta=$meta;canvas_presents=$canvasPresents;ui_presents=$uiPresents}
                    })
                    $sparseRuntime=$runtimeOnly -and $contacts.Count -eq 3 -and @($contacts|Where-Object {$_.canvas_presents -ne 1 -or $_.ui_presents -ne 0}).Count -eq 0
                    $intervalUnavailable=if($sparseRuntime){'Approved sparse runtime exception: interval timing is unverified when the owned trace records one canvas present and no UI presents'}else{$null}
                    $presentmonUnavailable=if($runtimeOnly){$runtimeReason}elseif($workload -eq 'startup' -and !$csvHasRows){'PresentMon 2.3.0 emitted no startup interval rows; the first present establishes the interval baseline'}else{$null}
                    $record=@{vm=$Vm;profile=$Profile;workload=$workload;arm=$arm;order_slot=$slot;order=$ArmOrder;no_capture=$noCapture;exit_code=$code;presentmon_exit_code=$collectorCode;presentmon_mode=$presentmonMode;presentmon_sha256=$PresentMonSha256;presentmon_unavailable_reason=$presentmonUnavailable;presentmon_interval_unavailable_reason=$intervalUnavailable;commit=$build.commit;binary_hashes=$build.files}
                    $record|ConvertTo-Json -Depth 8|Set-Content -LiteralPath (Join-Path $folder 'run.json') -Encoding UTF8
                    $records.Add($record)
                    foreach($contact in $contacts){
                        $meta=$contact.meta
                        foreach($key in @('vm','order_slot','order','commit','binary_hashes','presentmon_mode','presentmon_sha256','presentmon_unavailable_reason','presentmon_interval_unavailable_reason')){$meta|Add-Member -NotePropertyName $key -NotePropertyValue $record[$key] -Force}
                        $meta|ConvertTo-Json -Depth 8|Set-Content -LiteralPath $contact.file.FullName -Encoding UTF8
                    }
                }
                if($null -eq $collectorCode -or $collectorCode -ne 0){throw "PresentMon collector exited with code $collectorCode"}
                if(!$csvHasRows){
                    if($workload -eq 'startup' -or $sparseRuntime){continue}
                    throw 'PresentMon produced no interval rows'
                }
                if(!(Import-Csv -LiteralPath $csv|Select-Object -First 1)){throw 'PresentMon CSV contains no presents'}
                $headers=@((Get-Content -LiteralPath $csv -First 1) -split ','|ForEach-Object {$_.Trim().Trim([char]34)})
                $requiredHeaders=if($runtimeOnly){@('Application','ProcessID','Runtime','SwapChainAddress','msBetweenPresents','QPCTime')}else{@('PresentMode','SwapChainAddress','msBetweenPresents','msBetweenDisplayChange','msUntilDisplayed','QPCTime')}
                foreach($required in $requiredHeaders){if($required -notin $headers){throw "PresentMon CSV missing $required"}}
            }
        }
    }
    $overall=$true
}finally{
    @{ok=$overall;vm=$Vm;profile=$Profile;runs=$records}|ConvertTo-Json -Depth 10|Set-Content -LiteralPath (Join-Path $Out 'done.json') -Encoding UTF8
}
