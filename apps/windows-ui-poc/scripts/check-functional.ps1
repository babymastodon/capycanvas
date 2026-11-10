param(
    [Parameter(Mandatory)][ValidateSet('capycanvas6-poc-gpu','capycanvas6-poc-warp')][string]$Vm,
    [Parameter(Mandatory)][string]$Out,
    [string]$TargetDirectory,
    [ValidateSet('retained','winui','both')][string]$Arm='both',
    [string]$BenchmarkResults
)
$ErrorActionPreference='Stop'
$poc=(Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$repo=(Resolve-Path (Join-Path $poc '../..')).Path
$Out=[IO.Path]::GetFullPath($Out).TrimEnd('\')
[IO.Directory]::CreateDirectory($Out)|Out-Null
if(!$env:CAPY_PRIVATE_DESKTOP){& (Join-Path $repo 'apps/layer-windows/scripts/test-desktop.ps1') -Script $PSCommandPath -Parameters $PSBoundParameters -TimeoutSeconds 7200;return}
. (Join-Path $repo 'apps/layer-windows/scripts/CapyUia.ps1')
Add-Type -AssemblyName System.Drawing,System.Windows.Forms
if(!('CapyRowPointer' -as [type])){Add-Type -Path (Join-Path $repo 'apps/layer-windows/scripts/RowPointerDriver.cs')}
if(!('PocFunctionalNative' -as [type])){Add-Type -Path (Join-Path $PSScriptRoot 'FunctionalNative.cs') -ReferencedAssemblies System.Drawing}
[CapyRowPointer]::VerifyPrivateDesktop($env:CAPY_PRIVATE_DESKTOP)
[CapyRowPointer]::SetThreadDpiAwarenessContext([IntPtr]-4)|Out-Null
$script:CapyPopups=$false
if(!$TargetDirectory){$TargetDirectory=Join-Path $poc 'target'}
$bin=Join-Path $TargetDirectory 'release';$observer=Join-Path $bin 'poc-observer.exe';$dll=Join-Path $bin 'poc_canvas.dll'
$results=[Collections.Generic.List[object]]::new();$harnessErrors=[Collections.Generic.List[object]]::new()
$script:review=$null;$script:root=$null;$script:window=[IntPtr]::Zero
try{$spec=[PocFunctionalNative]::Spec($dll)|ConvertFrom-Json}catch{
    $reason=$_.Exception.Message
    foreach($selected in @(if($Arm -eq 'both'){'retained','winui'}else{$Arm})){foreach($i in 1..12){$results.Add(@{arm=$selected;id="F$i";status='unverified';error="Shared specification prerequisite failed: $reason";evidence=$dll})}}
    @{vm=$Vm;results=@($results);harness_errors=@(@{stage='shared specification';error=$reason})}|ConvertTo-Json -Depth 8|Set-Content -LiteralPath (Join-Path $Out 'functional.json') -Encoding UTF8
    throw
}
function Save-Results{
    @{classification='VM functional diagnostics';vm=$Vm;theme='dark';canvas_load='light';private_desktop=$env:CAPY_PRIVATE_DESKTOP;results=@($results);harness_errors=@($harnessErrors)}|ConvertTo-Json -Depth 24|Set-Content -LiteralPath (Join-Path $Out 'functional.json') -Encoding UTF8
}
function Unverified([string]$Reason,$Details=$null){$exception=[InvalidOperationException]::new($Reason);$exception.Data['unverified']=$true;$exception.Data['details']=$Details;throw $exception}
function Assert([bool]$Condition,[string]$Message){if(!$Condition){throw $Message}}
function Json-Lines([string]$Path){if(!(Test-Path -LiteralPath $Path)){return @()};@(Get-Content -LiteralPath $Path -Encoding UTF8|Where-Object {$_}|ForEach-Object {$_|ConvertFrom-Json})}
function Owned{
    Assert ($review -and !$review.HasExited) 'The recorded application process is not running'
    [PocFunctionalNative]::Own($window,[uint32]$review.Id)
    Assert ($root.Current.ProcessId -eq $review.Id) 'UI Automation root has a different owner'
}
function Foreground{
    Owned
    [CapyRowPointer]::SetForegroundWindow($window)|Out-Null
    Wait-Until {[CapyRowPointer]::GetForegroundWindow() -eq $window} 'The private test app did not acquire foreground input'
}
function Client-Origin{$point=[PocFunctionalNative+Point]::new();Assert ([PocFunctionalNative]::ClientToScreen($window,[ref]$point)) 'Client origin unavailable';$point}
function Bounds-Array($Element){$b=$Element.Current.BoundingRectangle;@($b.X,$b.Y,$b.Width,$b.Height)}
function Within-Viewport($Element,$Viewport,[switch]$Fully){[PocFunctionalNative]::BoundsVisible([double[]](Bounds-Array $Element),[double[]](Bounds-Array $Viewport),[bool]$Fully)}
function Pattern($Element,$Type){$value=$null;if(!$Element.TryGetCurrentPattern($Type::Pattern,[ref]$value)){throw "Missing UIA pattern $($Type.Name) on $($Element.Current.AutomationId)"};$value}
function Focus($Element){Foreground;$Element.SetFocus();Wait-Until {$Element.Current.HasKeyboardFocus} 'The owned control did not receive keyboard focus'}
function Key([uint16]$Code){Foreground;[CapyRowPointer]::Key([uint32]$review.Id,$Code)}
function Chord([uint16[]]$Modifiers,[uint16]$Code){Foreground;[CapyRowPointer]::Chord([uint32]$review.Id,$Modifiers,$Code)}
function Set-Filter([string]$Text){$entry=Control 'text-field';(Pattern $entry ([System.Windows.Automation.ValuePattern])).SetValue($Text);Wait-Until {(Pattern $entry ([System.Windows.Automation.ValuePattern])).Current.Value -ceq $Text} 'Filter Value did not update'}
function Props-Scroll([double]$Percent){
    $properties=Control 'properties';$available=[System.Windows.Automation.PropertyCondition]::new([System.Windows.Automation.AutomationElement]::IsScrollPatternAvailableProperty,$true)
    $providers=@($properties.FindAll([System.Windows.Automation.TreeScope]::Descendants,$available))
    foreach($provider in $providers){$scroll=Pattern $provider ([System.Windows.Automation.ScrollPattern]);if($scroll.Current.VerticallyScrollable){$scroll.SetScrollPercent([System.Windows.Automation.ScrollPattern]::NoScroll,$Percent);return}}
}
function Row([uint32]$Index){
    $list=Control 'layers-list';$id=[PocFunctionalNative]::RowId($dll,$Index)
    $item=Find $id -Within $list
    if(!$item){$container=$null;if($list.TryGetCurrentPattern([System.Windows.Automation.ItemContainerPattern]::Pattern,[ref]$container)){$item=$container.FindItemByProperty($null,[System.Windows.Automation.AutomationElement]::AutomationIdProperty,$id)}}
    Assert ($null -ne $item) "Missing virtual row $id in layers-list"
    Assert ($item.Current.ProcessId -eq $review.Id) 'Virtual row belongs to another process'
    $scroll=Pattern $item ([System.Windows.Automation.ScrollItemPattern]);$scroll.ScrollIntoView()
    Wait-Until {Within-Viewport $item $list -Fully} "$id bounds did not enter the layers-list viewport"
    $item
}
function Virtual-Row([uint32]$Index,$Steps){
    $list=Control 'layers-list';$id=[PocFunctionalNative]::RowId($dll,$Index)
    $Steps.Add(@{step='ItemContainerPattern';status='attempt'});$container=Pattern $list ([System.Windows.Automation.ItemContainerPattern]);$Steps[-1].status='pass'
    $Steps.Add(@{step='FindItemByProperty';property='AutomationId';value=$id;status='attempt'});$item=$container.FindItemByProperty($null,[System.Windows.Automation.AutomationElement]::AutomationIdProperty,$id);Assert ($null -ne $item) "ItemContainerPattern.FindItemByProperty returned no item for $id";$Steps[-1].status='pass'
    $Steps.Add(@{step='VirtualizedItemPattern';status='attempt'});$virtual=Pattern $item ([System.Windows.Automation.VirtualizedItemPattern]);$Steps[-1].status='pass'
    $Steps.Add(@{step='Realize';status='attempt'});$virtual.Realize();$Steps[-1].status='pass'
    Assert ($item.Current.ProcessId -eq $review.Id) 'Realized row belongs to another process'
    $Steps.Add(@{step='ScrollItemPattern.ScrollIntoView';status='attempt'});$scroll=Pattern $item ([System.Windows.Automation.ScrollItemPattern]);$scroll.ScrollIntoView();Wait-Until {Within-Viewport $item $list -Fully} "$id bounds did not enter the layers-list viewport";$Steps[-1].status='pass'
    $item
}
function Wait-Observer([Diagnostics.Process]$Process,[string]$Evidence){
    if($Process.WaitForExit(180000)){return}
    @{error='Observer exceeded 180 seconds';observer_pid=$Process.Id;observer_started=$Process.StartTime.ToUniversalTime().ToString('o');graceful=$false}|ConvertTo-Json|Set-Content -LiteralPath $Evidence -Encoding UTF8
    try{
        $children=@(Get-CimInstance Win32_Process -Filter ("ParentProcessId = "+[uint32]$Process.Id)|Where-Object {$_.ExecutablePath -ieq $executable})
        foreach($native in $children){
            $child=[Diagnostics.Process]::GetProcessById([int]$native.ProcessId);$null=$child.Handle
            if($child.StartTime -ge $Process.StartTime -and !$child.HasExited){
                @{observer_pid=$Process.Id;process_id=$child.Id;executable=$native.ExecutablePath;parent_process_id=$native.ParentProcessId;started=$child.StartTime.ToUniversalTime().ToString('o');action='terminate exact observer-owned child after timeout';graceful=$false}|ConvertTo-Json|Set-Content -LiteralPath ($Evidence+'.owned-child-'+$child.Id+'.json') -Encoding UTF8
                $child.Kill();Assert ($child.WaitForExit(5000)) 'Recorded observer-owned app did not terminate'
            }
        }
    }catch{$_|Out-String|Set-Content -LiteralPath ($Evidence+'.cleanup-error.txt') -Encoding UTF8}finally{
        if(!$Process.HasExited){$Process.Kill();Assert ($Process.WaitForExit(5000)) 'Recorded observer process did not terminate'}
    }
    throw "Observer exceeded 180 seconds; failure preserved at $Evidence"
}
function Place-Ui([int]$Width,[int]$Height,[int]$X,[int]$Y){
    Owned;[PocFunctionalNative]::ResizeClient($window,[uint32]$review.Id,$Width,$Height,$X,$Y)
    $client=[PocFunctionalNative+Rect]::new();$outer=[PocFunctionalNative+Rect]::new();[PocFunctionalNative]::GetClientRect($window,[ref]$client)|Out-Null;[PocFunctionalNative]::GetWindowRect($window,[ref]$outer)|Out-Null
    $screen=[Windows.Forms.Screen]::PrimaryScreen.Bounds
    Assert ($outer.Left -ge $screen.Left -and $outer.Top -ge $screen.Top -and $outer.Right -le $screen.Right -and $outer.Bottom -le $screen.Bottom) 'Controlled application placement is clipped by the private primary display'
    @{logical=@($Width,$Height);physical=@($client.Width,$client.Height);outer=@($outer.Left,$outer.Top,$outer.Width,$outer.Height);scale=([PocFunctionalNative]::GetDpiForWindow($window)/96.0)}
}
function Snapshot([string]$Name,$Element=$null,[switch]$Screen){
    Owned
    $path=Join-Path $armOut ($Name+'.png');$arguments=@('snapshot','--hwnd',$window.ToInt64(),'--pid',$review.Id,'--out',$path)
    if($Screen){$arguments+='--screen'}elseif($Element){$b=$Element.Current.BoundingRectangle;$origin=Client-Origin;$client=[PocFunctionalNative+Rect]::new();Assert ([PocFunctionalNative]::GetClientRect($window,[ref]$client)) 'Client rectangle unavailable';$x=[int][Math]::Max(0,[Math]::Floor($b.X-$origin.X));$y=[int][Math]::Max(0,[Math]::Floor($b.Y-$origin.Y));$w=[int][Math]::Min([Math]::Ceiling($b.Right-$origin.X)-$x,$client.Width-$x);$h=[int][Math]::Min([Math]::Ceiling($b.Bottom-$origin.Y)-$y,$client.Height-$y);Assert ($w -gt 0 -and $h -gt 0) 'Requested crop is outside the owned client';$arguments+=@('--rect',("$x,$y,$w,$h"))}
    $stdout=Join-Path $armOut ($Name+'-snapshot.log');$stderr=Join-Path $armOut ($Name+'-snapshot-error.log')
    $process=Start-Process -FilePath $observer -ArgumentList (@($arguments|ForEach-Object {'"'+[string]$_+'"'}) -join ' ') -PassThru -RedirectStandardOutput $stdout -RedirectStandardError $stderr;$null=$process.Handle
    Wait-Observer $process (Join-Path $armOut ($Name+'-snapshot-timeout.json'))
    $code=$process.ExitCode;$text=Get-Content -LiteralPath $stdout
    Assert ($code -eq 0) "Desktop Duplication snapshot failed ($code): $Name"
    $meta=($text -join "`n")|ConvertFrom-Json;$meta|ConvertTo-Json -Depth 8|Set-Content -LiteralPath (Join-Path $armOut ($Name+'-snapshot.json')) -Encoding UTF8
    @{path=$path;metadata=$meta}
}
function Start-Ui{
    $script:uiClosedGracefully=$false
    $script:uiTrace=Join-Path $armOut 'ui-canvas.jsonl'
    $arguments=@('--rows','1000','--canvas-load','light','--thumbnails','off','--trace',('"'+$uiTrace+'"'))
    $script:review=Start-Process -FilePath $executable -ArgumentList $arguments -WorkingDirectory $armOut -PassThru -RedirectStandardError (Join-Path $armOut 'ui-error.log');$null=$review.Handle
    Wait-Until {$handles=@([PocFunctionalNative]::Windows([uint32]$review.Id)|Where-Object {$candidate=[System.Windows.Automation.AutomationElement]::FromHandle($_);$candidate.Current.Name -eq $spec.title}|Select-Object -First 1);if(!$handles.Count){return $false};$script:window=$handles[0];$window -ne [IntPtr]::Zero} 'The owned POC window did not appear' 60
    $script:root=[System.Windows.Automation.AutomationElement]::FromHandle($window);Foreground
    $screen=[Windows.Forms.Screen]::PrimaryScreen.Bounds;$geometry=Place-Ui $spec.client[0] $spec.client[1] ($screen.Left+16) ($screen.Top+16);$geometry|ConvertTo-Json -Depth 4|Set-Content -LiteralPath (Join-Path $armOut 'ui-placement.json') -Encoding UTF8
    $review.Id|Set-Content -LiteralPath (Join-Path $armOut 'owned-ui-pid.txt')
    [CapyRowPointer]::Initialize([uint32]$review.Id)
    Control 'layers-list' -Arranged|Out-Null
}
function Close-Ui{
    try{
        [CapyRowPointer]::Dispose()
        if($review){
            Assert (!$review.HasExited) 'Owned UI application exited before graceful close'
            [PocFunctionalNative]::Close($window,[uint32]$review.Id);Assert ($review.WaitForExit(15000)) 'Owned application did not close gracefully';Assert ($review.ExitCode -eq 0) "Owned application exited $($review.ExitCode)"
            $script:uiClosedGracefully=$true
        }
    }catch{
        $failure=$_;$harnessErrors.Add(@{arm=$which;stage='owned application close';process_id=$(if($review){$review.Id}else{$null});error=$failure.Exception.Message;graceful=$false;repro=$repro});Save-Results
        if($review -and !$review.HasExited){try{$review.Kill();Assert ($review.WaitForExit(5000)) 'Exact owned application did not terminate'}catch{$harnessErrors.Add(@{arm=$which;stage='exact owned process termination';process_id=$review.Id;error=$_.Exception.Message});Save-Results}}
        throw $failure
    }finally{$script:review=$null;$script:root=$null;$script:window=[IntPtr]::Zero}
}
function Check([string]$Id,[string]$Name,[scriptblock]$Action){
    $started=[DateTime]::UtcNow;$watch=[Diagnostics.Stopwatch]::StartNew();$record=[ordered]@{arm=$which;id=$Id;name=$Name;status='fail';started_utc=$started.ToString('o');repro=$repro;evidence_directory=$armOut;details=$null}
    try{$details=& $Action;$record.details=$details;$record.status=if($details.status){$details.status}else{'pass'}}catch{$record.error=$_.Exception.Message;$record.status=if($_.Exception.Data['unverified']){'unverified'}else{'fail'};$record.details=$_.Exception.Data['details'];$record.script_stack=$_.ScriptStackTrace;if($review -and !$review.HasExited){try{$record.failure_snapshot=(Snapshot ($Id+'-failure')).path}catch{$record.failure_snapshot_error=$_.Exception.Message}}}
    $record.elapsed_ms=$watch.Elapsed.TotalMilliseconds;$results.Add([pscustomobject]$record);Save-Results
}
function Contact-Completed([string]$Directory){
    $path=Join-Path $Directory 'completed.json'
    if(!(Test-Path -LiteralPath $path) -or (Test-Path -LiteralPath (Join-Path $Directory 'failures.json'))){return $false}
    $completed=Get-Content -LiteralPath $path -Raw|ConvertFrom-Json
    $completed.completed -eq $true -and $completed.closed_gracefully -eq $true
}
function Benchmark-Completed([string]$Directory,$Meta){
    if(!(Contact-Completed $Directory)){return $false}
    $runPath=Join-Path (Split-Path -Parent $Directory) 'run.json'
    if(!(Test-Path -LiteralPath $runPath)){return $false}
    $run=Get-Content -LiteralPath $runPath -Raw|ConvertFrom-Json
    if($run.exit_code -ne 0 -or $null -eq $run.exit_code -or $run.presentmon_exit_code -ne 0 -or $null -eq $run.presentmon_exit_code){return $false}
    foreach($key in @('arm','workload','profile','presentmon_mode')){if($run.$key -ne $Meta.$key){return $false}}
    $cursor=[IO.DirectoryInfo]::new($Directory)
    while($cursor){$donePath=Join-Path $cursor.FullName 'done.json';if(Test-Path -LiteralPath $donePath){$done=Get-Content -LiteralPath $donePath -Raw|ConvertFrom-Json;return ($done.ok -eq $true -and @($done.runs|Where-Object {$_.arm -eq $Meta.arm -and $_.workload -eq $Meta.workload -and $_.profile -eq $Meta.profile -and $_.exit_code -eq 0 -and $_.presentmon_mode -eq $Meta.presentmon_mode}).Count -gt 0)};$cursor=$cursor.Parent}
    $false
}
function Matching-Binary($Meta){
    $files=@($executable,$observer);if($which -eq 'winui'){$files+=$dll}
    foreach($file in $files){$hash=@($Meta.binary_hashes|Where-Object {$_.path -ieq [IO.Path]::GetFileName($file)});if($hash.Count -ne 1 -or $hash[0].sha256 -ine (Get-FileHash -LiteralPath $file -Algorithm SHA256).Hash){return $false}}
    $true
}
function Observed([string]$Workload,[int]$Rows=1000){
    if($BenchmarkResults -and (Test-Path -LiteralPath $BenchmarkResults -PathType Container)){
        $found=@(Get-ChildItem -LiteralPath $BenchmarkResults -Filter meta.json -Recurse -File|Where-Object {$m=Get-Content -LiteralPath $_.FullName -Raw|ConvertFrom-Json;$n=1000;$appArguments=@($m.app_args);$at=[array]::IndexOf($appArguments,'--rows');if($at -ge 0){$n=[int]$appArguments[$at+1]};$m.arm -eq $which -and $m.workload -eq $Workload -and $n -eq $Rows -and !$m.no_capture -and (Benchmark-Completed $_.DirectoryName $m) -and (Matching-Binary $m)})
        if($found.Count){return @($found|ForEach-Object {$_.DirectoryName})}
    }
    $destination=Join-Path $armOut ("observe-$Workload-$Rows");$arguments=@('run','--arm',$which,'--exe',$executable,'--workload',$Workload,'--contacts','1','--seconds','6','--warmup','1','--profile','normal','--out',$destination,'--','--rows',$Rows,'--canvas-load','light','--thumbnails',$(if($Workload -in 'scroll-wheel','scroll-touch'){'on'}else{'off'}))
    $process=Start-Process -FilePath $observer -ArgumentList (@($arguments|ForEach-Object {'"'+[string]$_+'"'}) -join ' ') -PassThru -RedirectStandardOutput (Join-Path $armOut ("$Workload-$Rows-observer.log")) -RedirectStandardError (Join-Path $armOut ("$Workload-$Rows-observer-error.log"));$null=$process.Handle;Wait-Observer $process (Join-Path $armOut ("$Workload-$Rows-timeout.json"));Assert ($process.ExitCode -eq 0) "Functional observer $Workload/$Rows exited $($process.ExitCode)";$contact=Join-Path $destination 'contact-000';Assert (Contact-Completed $contact) 'Functional contact lacks successful graceful completion evidence';@($contact)
}
function Read-ImeValue([scriptblock]$ReadValue,[scriptblock]$Accept,[string]$Message){
    $accepted=@{text=$null}
    Wait-Until {$text=& $ReadValue;if(!(& $Accept $text)){return $false};$accepted.text=$text;$true} $Message 15
    $accepted.text
}
function Caret-Rectangle($Entry){
    $pattern=$null
    $caretType='System.Windows.Automation.TextPattern2' -as [type]
    if($caretType){try{if($Entry.TryGetCurrentPattern($caretType::Pattern,[ref]$pattern)){$active=$false;$range=$pattern.GetCaretRange([ref]$active);$rects=$range.GetBoundingRectangles();if($rects.Length -ge 4 -and $rects[3] -gt 0){return @{rect=@($rects[0],$rects[1],[Math]::Max(1,$rects[2]),$rects[3]);line_height=$rects[3];source='UIA TextPattern2'}}}}catch [System.NotSupportedException]{}}
    $native=[PocFunctionalNative]::Caret($window,[uint32]$review.Id);if($native -and $native[3] -gt 0){return @{rect=$native;line_height=$native[3];source='GetGUIThreadInfo'}}

    $null
}
function Assess-Candidate($Evidence,$Caret){
    $Evidence.caret=$Caret;$Evidence.deferred_caret=$false
    $origin=$Evidence.capture_origin;$pngCaret=@(($Caret.rect[0]-$origin[0]),($Caret.rect[1]-$origin[1]),$Caret.rect[2],$Caret.rect[3])
    $regions=[PocFunctionalPixels+Region[]]$Evidence.regions
    $indices=@([PocFunctionalPixels]::CandidateRegions($regions,[double[]]$pngCaret,[double]$Caret.line_height))
    $Evidence.selection_rule='All connected regions at least one actual caret line height in each dimension and with nearest edge within two line heights; no closest-region ranking'
    $Evidence.candidate_indices=$indices;$Evidence.visual_review='required';$Evidence.geometry_status='unverified'
    for($i=0;$i -lt $regions.Length;$i++){$region=$regions[$i];$Evidence.region_analysis+=@(@{index=$i;screen_rect=@(($region.X+$origin[0]),($region.Y+$origin[1]),$region.Width,$region.Height);changed_pixels=$region.ChangedPixels;nearest_caret_edge_px=[PocFunctionalPixels]::EdgeDistance([double[]]@($region.X,$region.Y,$region.Width,$region.Height),[double[]]$pngCaret);candidate=($i -in $indices)})}
    if($indices.Count -ne 1){$Evidence.selection_status=if($indices.Count){'ambiguous'}else{'no candidate component'};return}
    $selected=$regions[$indices[0]];$Evidence.selection_status='one geometric candidate; visual confirmation required';$Evidence.geometry_status='within two line heights';$Evidence.candidate_rect=@(($selected.X+$origin[0]),($selected.Y+$origin[1]),$selected.Width,$selected.Height);$Evidence.selected_candidate_index=$indices[0];$Evidence.candidate_edge_distance_px=[PocFunctionalPixels]::EdgeDistance([double[]]@($selected.X,$selected.Y,$selected.Width,$selected.Height),[double[]]$pngCaret)
    $Evidence.before_crop=Join-Path $armOut 'F4-candidate-before.png';$Evidence.after_crop=Join-Path $armOut 'F4-candidate-after.png'
    [PocFunctionalPixels]::Crop($Evidence.before,$Evidence.before_crop,$selected.X,$selected.Y,$selected.Width,$selected.Height);[PocFunctionalPixels]::Crop($Evidence.after,$Evidence.after_crop,$selected.X,$selected.Y,$selected.Width,$selected.Height)
}
function Resolve-DeferredCaret{
    if(!$uiClosedGracefully){return}
    foreach($item in @($results|Where-Object {$_.arm -eq $which -and $_.id -eq 'F4' -and $_.status -eq 'unverified' -and $_.details.deferred_caret})){
        $evidence=$item.details;$carets=@(Json-Lines $uiTrace|Where-Object {$_.kind -eq 'caret' -and $_.qpc -le $evidence.before_capture_qpc -and $_.rect.Count -eq 4 -and $_.line_height -gt 0}|Sort-Object qpc|Select-Object -Last 1)
        if(!$carets.Count){$evidence.deferred_caret_reason='Gracefully flushed trace contains no actual caret at or before the before-Space capture';continue}
        $actual=$carets[0];$client=$evidence.client_origin;$rect=@(($actual.rect[0]+$client[0]),($actual.rect[1]+$client[1]),$actual.rect[2],$actual.rect[3])
        Assess-Candidate $evidence @{rect=$rect;line_height=$actual.line_height;source='flushed caret trace';qpc=$actual.qpc};$item.error='IME text gates passed; masked candidate detection requires visual review: '+$evidence.selection_status
    }
    Save-Results
}
function Text-LayoutEvidence($Trace,$Spec){
    $labels=@();$matched=@();$absent=@()
    foreach($label in $Spec.labels){$records=@($Trace|Where-Object {$_.kind -eq 'text_layout' -and $_.text -ceq $label -and $_.wrap -eq $false});$labels+=@(@{text=$label;records=$records});$matched+=$records;if(!$records.Count){$absent+=@($label)}}
    $paragraph=@($Trace|Where-Object {$_.kind -eq 'text_layout' -and $_.text -ceq $Spec.paragraph -and $_.wrap -eq $true});$matched+=$paragraph;if(!$paragraph.Count){$absent+=@($Spec.paragraph)}
    $incomplete=@($matched|Where-Object {$null -eq $_.missing_glyphs -or $null -eq $_.line_breaks});$missing=0
    foreach($record in $matched){if($null -ne $record.missing_glyphs){$missing+=[int64]$record.missing_glyphs}}
    $status=if($missing -gt 0){'fail'}elseif($absent.Count -or $incomplete.Count){'unverified'}else{'observed'}
    @{status=$status;glyph_status=$(if($missing -gt 0){'fail'}elseif($status -eq 'observed'){'pass'}else{'unverified'});missing_glyphs=$missing;missing_text_records=$absent;incomplete_records=$incomplete;labels=$labels;paragraph=$paragraph;line_break_offset_basis='Native shaping trace UTF-8 byte offsets';thai_word_boundaries='unverified; review the actual offsets and crops';visual_review='required'}
}
function Resolve-DeferredText{
    if($which -ne 'retained' -or !$uiClosedGracefully){return}
    $evidence=Text-LayoutEvidence (Json-Lines $uiTrace) $spec;$path=Join-Path $armOut 'F3-text-layout.json';$evidence|ConvertTo-Json -Depth 18|Set-Content -LiteralPath $path -Encoding UTF8
    foreach($item in @($results|Where-Object {$_.arm -eq $which -and $_.id -eq 'F3'})){
        if(!$item.details){$item.details=@{}}
        $item.details.text_layout=$evidence;$item.details.text_layout_path=$path
        if($evidence.status -eq 'fail'){$item.status='fail';$item|Add-Member -NotePropertyName error -NotePropertyValue ("Retained text trace records "+$evidence.missing_glyphs+' missing glyphs; see '+$path) -Force}
        elseif($item.status -ne 'fail'){$item.status='unverified';$item|Add-Member -NotePropertyName error -NotePropertyValue $(if($evidence.status -eq 'unverified'){'Retained text trace is incomplete; glyph counts and Thai break offsets remain unverified'}else{'Retained trace has zero missing glyphs for all seven labels and wrapped paragraph; Thai word boundaries and glyph forms require visual review'}) -Force}
    }
    Save-Results
}
function Maximize-Bounds{
    foreach($name in @('Maximize','MaximizeButton','Restore','RestoreButton')){$item=$root.FindFirst([System.Windows.Automation.TreeScope]::Descendants,[System.Windows.Automation.PropertyCondition]::new([System.Windows.Automation.AutomationElement]::AutomationIdProperty,$name));if($item -and !$item.Current.IsOffscreen){return Bounds-Array $item}}
    [PocFunctionalNative]::MaximizeButton($window,[uint32]$review.Id)
}
function Tap([double[]]$Bounds){Foreground;[CapyRowPointer]::Down('mouse',[int]($Bounds[0]+$Bounds[2]/2),[int]($Bounds[1]+$Bounds[3]/2));[CapyRowPointer]::Up()}
function Popup-Items{
    $found=[Collections.Generic.List[object]]::new()
    foreach($id in $spec.menu_ids){$item=$null;foreach($handle in [PocFunctionalNative]::Windows([uint32]$review.Id)){$surface=[System.Windows.Automation.AutomationElement]::FromHandle($handle);$item=Find $id -Within $surface;if($item){break}};Assert ($null -ne $item) "Missing owned popup item $id";$found.Add($item)}
    @($found)
}
function Menu-Control{
    Props-Scroll 0;$menu=Control 'menu-button';$viewport=Control 'properties'
    if(!(Within-Viewport $menu $viewport)){(Pattern $menu ([System.Windows.Automation.ScrollItemPattern])).ScrollIntoView()}
    Wait-Until {Within-Viewport $menu $viewport} 'Menu bounds did not enter the properties viewport after ScrollItem'
    Control 'menu-button' -Arranged
}
function Popup-Container($Items){
    $walker=[System.Windows.Automation.TreeWalker]::ControlViewWalker;$rootIdentity=$root.GetRuntimeId() -join ','
    for($candidate=$walker.GetParent($Items[0]);$candidate;$candidate=$walker.GetParent($candidate)){
        if($candidate.Current.ProcessId -ne $review.Id -or ($candidate.GetRuntimeId() -join ',') -eq $rootIdentity){break}
        $kind=$candidate.Current.ControlType
        if($kind -ne [System.Windows.Automation.ControlType]::Menu -and ($kind -ne [System.Windows.Automation.ControlType]::Window -or [IntPtr]$candidate.Current.NativeWindowHandle -eq $window)){continue}
        $identity=$candidate.GetRuntimeId() -join ',';$common=$true
        foreach($item in $Items){$inside=$false;for($ancestor=$walker.GetParent($item);$ancestor;$ancestor=$walker.GetParent($ancestor)){if(($ancestor.GetRuntimeId() -join ',') -eq $identity){$inside=$true;break};if($ancestor.Current.ProcessId -ne $review.Id){break}};if(!$inside){$common=$false;break}}
        if(!$common){continue}
        $bounds=$candidate.Current.BoundingRectangle
        if($bounds.IsEmpty -or $bounds.Width -le 0 -or $bounds.Height -le 0){continue}
        return @{bounds=(Bounds-Array $candidate);source='owned common UIA popup container';control_type=$kind.ProgrammaticName;automation_id=$candidate.Current.AutomationId;class_name=$candidate.Current.ClassName;native_window_handle=$candidate.Current.NativeWindowHandle}
    }
    $native=@([PocFunctionalNative]::Windows([uint32]$review.Id)|Where-Object {$_ -ne $window -and [PocFunctionalNative]::ClassName($_,[uint32]$review.Id) -ceq 'CapyCanvasPocRetainedPopup'})
    if($native.Count -eq 1){$handle=$native[0];[PocFunctionalNative]::Own($handle,[uint32]$review.Id);$bounds=[PocFunctionalNative+Rect]::new();Assert ([PocFunctionalNative]::GetWindowRect($handle,[ref]$bounds)) 'Owned retained popup rectangle unavailable';if($bounds.Width -gt 0 -and $bounds.Height -gt 0){return @{bounds=@($bounds.Left,$bounds.Top,$bounds.Width,$bounds.Height);source='owned native popup HWND';class_name='CapyCanvasPocRetainedPopup';native_window_handle=$handle.ToInt64()}}}
    $null
}
function Shared-PatternCheck([string]$Name,[scriptblock]$Action,$Rows){try{$value=&$Action;$Rows.Add(@{pattern=$Name;status='pass';value=$value})}catch{$Rows.Add(@{pattern=$Name;status='fail';error=$_.Exception.Message})}}
$arms=@(if($Arm -eq 'both'){'retained','winui'}else{$Arm})
foreach($which in $arms){
    $armOut=Join-Path $Out $which;[IO.Directory]::CreateDirectory($armOut)|Out-Null
    $executable=Join-Path $bin (@{retained='poc-retained.exe';winui='PocWinUI.exe'}[$which])
    $repro="check-functional.ps1 -Vm $Vm -Arm $which -Out `"$Out`" -TargetDirectory `"$TargetDirectory`""
    if(!(Test-Path -LiteralPath $executable)){foreach($i in 1..12){Check "F$i" 'Application prerequisite' {Unverified "The $which executable has not been built: $executable"}};continue}
    Check 'F1' 'Present modes' {
        Unverified 'Approved runtime-only collector exceptions: PresentMode and display timing are unverified on both VMs' @{present_modes=$null;presentmon_mode='runtime-only'}
    }
    Check 'F2' 'Pen samples, axes and history' {
        $destination=Join-Path $armOut 'pen-check';$arguments=@('pen-check','--arm',$which,'--exe',$executable,'--out',$destination,'--samples','2400','--','--rows','1000','--canvas-load','light','--thumbnails','off')
        $process=Start-Process -FilePath $observer -ArgumentList (@($arguments|ForEach-Object {'"'+[string]$_+'"'}) -join ' ') -PassThru -RedirectStandardOutput (Join-Path $armOut 'pen-check.log') -RedirectStandardError (Join-Path $armOut 'pen-check-error.log');$null=$process.Handle;Wait-Observer $process (Join-Path $armOut 'pen-check-timeout.json');Assert ($process.ExitCode -eq 0) "Pen-check exited $($process.ExitCode)"
        $contact=Join-Path $destination 'contact-000';$delivery=Get-Content -LiteralPath (Join-Path $contact 'pen-delivery.json') -Raw|ConvertFrom-Json
        Assert ($delivery.validated -and $delivery.matching_basis -eq 'ordered_indices_and_quantized_sample_tuples') 'Pen identity was not validated by ordered sample tuples'
        foreach($name in @('injected','received','consumed')){Assert ($delivery.$name -eq 2400) "Pen $name count is $($delivery.$name), expected 2400"}
        $inputs=@(Json-Lines (Join-Path $contact 'inputs.jsonl')|Where-Object kind -eq 'pen');$trace=@(Json-Lines (Join-Path $contact 'canvas.jsonl'));$consumed=@($trace|Where-Object kind -eq 'pen_consumed');Assert ($inputs.Count -eq 2400 -and $consumed.Count -eq 2400) 'Raw injected and consumed sample counts differ from 2400'
        $axes=@{pressure=0.0;tilt_x=0.0;tilt_y=0.0;twist=0.0}
        for($i=0;$i -lt 2400;$i++){Assert ($inputs[$i].index -eq $i -and $consumed[$i].index -eq $i) 'Pen source order differs from received order';foreach($axis in @('pressure','tilt_x','tilt_y','twist')){$axes[$axis]=[Math]::Max($axes[$axis],[Math]::Abs([double]$inputs[$i].$axis-[double]$consumed[$i].sample.$axis))}}
        Assert ($axes.pressure -le .01 -and $axes.tilt_x -le .9 -and $axes.tilt_y -le .9 -and $axes.twist -le 3.6) 'Pen axes exceed 1% of their full ranges'
        $history=@($trace|Where-Object kind -eq 'pen_history');$maximum=($history|Measure-Object samples -Maximum).Maximum;Assert ($maximum -gt 1) 'No actual OS input message delivered more than one historical sample'
        @{status='pass';delivery=$delivery;max_axis_error=$axes;history_messages=$history.Count;max_history_samples=$maximum;evidence=$contact;injection=(Get-Content -LiteralPath (Join-Path $contact 'injection.json') -Raw|ConvertFrom-Json)}
    }
    try{
        Start-Ui
        Check 'F3' 'Multilingual labels and wrapped text' {
            Set-Filter '';$crops=[Collections.Generic.List[object]]::new()
            for($index=0;$index -le 60;$index+=10){
                $expected=$spec.labels[$index/10]
                try{$item=Row $index;$text=$item.FindFirst([System.Windows.Automation.TreeScope]::Descendants,[System.Windows.Automation.PropertyCondition]::new([System.Windows.Automation.AutomationElement]::NameProperty,$expected));Assert ($text -or $item.Current.Name.Contains($expected)) "Row $index does not expose its shared multilingual label";$shot=Snapshot ("F3-row-$index") $item;$crops.Add(@{row=$index;label=$expected;status='pass';crop=$shot.path})}catch{$crops.Add(@{row=$index;label=$expected;status='fail';error=$_.Exception.Message})}
            }
            Props-Scroll 100;$wrap=Control 'wrap-text' -Arranged;$paragraph=Snapshot 'F3-wrap-text' $wrap
            if(@($crops|Where-Object status -eq 'fail').Count){return @{status='fail';labels=@($crops);paragraph=$paragraph.path}}
            Unverified 'Text crops require visual review for glyph forms and Thai word boundaries' @{labels=@($crops);paragraph=$paragraph.path;ui_strings='verified';visual_review='required'}
        }
        Check 'F4' 'Microsoft Japanese IME and candidate placement' {
            Props-Scroll 60;$entry=Control 'text-field';Focus $entry
            if(!(Get-WinUserLanguageList|Where-Object {$_.InputMethodTips -match '^0411:'})){Unverified 'Microsoft Japanese IME profile is not installed for the private test account'}
            if([PocFunctionalNative]::Language($window,[uint32]$review.Id) -ne 0x411){Chord ([uint16[]]@(91)) 32}
            Wait-Until {[PocFunctionalNative]::Language($window,[uint32]$review.Id) -eq 0x411} 'The owned field did not activate the Japanese keyboard profile' 15
            $evidence=[ordered]@{}
            try{
                [PocFunctionalNative]::ImeOpen($window,[uint32]$review.Id,$true);Chord ([uint16[]]@(17)) 20;Chord ([uint16[]]@(17)) 65
                foreach($character in 'NIHONGO'.ToCharArray()){Key ([uint16]$character)}
                $evidence.preedit=Read-ImeValue {(Pattern $entry ([System.Windows.Automation.ValuePattern])).Current.Value} {param($text)$text -ceq 'にほんご'} 'Japanese IME did not produce genuine にほんご preedit'
                $caret=Caret-Rectangle $entry;$field=Bounds-Array $entry;$clientOrigin=Client-Origin;$client=[PocFunctionalNative+Rect]::new();[PocFunctionalNative]::GetClientRect($window,[ref]$client)|Out-Null
                $before=Snapshot 'F4-before-space' -Screen;$evidence.before=$before.path;$evidence.before_capture_qpc=$before.metadata.qpc;$evidence.before_present_qpc=$before.metadata.last_present_qpc;$evidence.client_origin=@($clientOrigin.X,$clientOrigin.Y);$evidence.field_screen_rect=$field
                $clock=[Diagnostics.Stopwatch]::StartNew();$evidence.before_space_qpc=[Diagnostics.Stopwatch]::GetTimestamp();Key 32;$captured=[Collections.Generic.List[object]]::new();$number=0
                while($clock.ElapsedMilliseconds -le 500){$number++;$candidate=Snapshot ("F4-after-space-$number") -Screen;$captureMs=1000.0*($candidate.metadata.last_present_qpc-$evidence.before_space_qpc)/[Diagnostics.Stopwatch]::Frequency;if($candidate.metadata.last_present_qpc -gt 0 -and $captureMs -ge 0 -and $captureMs -le 500){$captured.Add(@{shot=$candidate;capture_ms=$captureMs;acquisition_ms=(1000.0*($candidate.metadata.qpc-$evidence.before_space_qpc)/[Diagnostics.Stopwatch]::Frequency)})}}
                $evidence.candidate_text=Read-ImeValue {(Pattern $entry ([System.Windows.Automation.ValuePattern])).Current.Value} {param($text)$text -match '[\u3400-\u9fff]'} 'Japanese IME did not expose a converted kanji candidate';Key 13
                $evidence.committed=Read-ImeValue {(Pattern $entry ([System.Windows.Automation.ValuePattern])).Current.Value} {param($text)$text.Contains('日本語')} 'Committed IME text does not contain 日本語'
                if(!$captured.Count){Unverified 'No Desktop Duplication candidate frame was captured within 500 ms after Space' $evidence}
                $evidence.frames=@();$marker=$spec.marker;$ui=@(($marker.ui_origin[0]-1),($marker.ui_origin[1]-1),($marker.width+2),($marker.cell+2));$canvas=@(($marker.canvas_origin_x-1),($client.Height-$marker.canvas_bottom-1),($marker.width+2),($marker.cell+2))
                foreach($frame in $captured){
                    $after=$frame.shot;$origin=@($after.metadata.origin);$fieldLeft=[Math]::Floor($field[0]-$origin[0]);$fieldTop=[Math]::Floor($field[1]-$origin[1]);$fieldWidth=[Math]::Ceiling($field[0]+$field[2]-$origin[0])-$fieldLeft;$fieldHeight=[Math]::Ceiling($field[1]+$field[3]-$origin[1])-$fieldTop;$masks=[int[]]@($fieldLeft,$fieldTop,$fieldWidth,$fieldHeight,($clientOrigin.X+$ui[0]-$origin[0]),($clientOrigin.Y+$ui[1]-$origin[1]),$ui[2],$ui[3],($clientOrigin.X+$canvas[0]-$origin[0]),($clientOrigin.Y+$canvas[1]-$origin[1]),$canvas[2],$canvas[3])
                    $regions=@([PocFunctionalPixels]::Regions($before.path,$after.path,$masks));$raw=[PocFunctionalPixels]::Difference($before.path,$after.path,0,0,0,0);$rawImage=$after.path.Replace('.png','-raw-diff.png');[PocFunctionalPixels]::DifferenceImage($before.path,$after.path,$rawImage)
                    $evidence.frames+=@(@{after=$after.path;capture_ms=$frame.capture_ms;acquisition_ms=$frame.acquisition_ms;qpc=$after.metadata.qpc;last_present_qpc=$after.metadata.last_present_qpc;raw_diff=$raw;raw_diff_image=$rawImage;masks=$masks;regions=$regions;capture_origin=$origin})
                }
                $latest=$evidence.frames[-1];$evidence.after=$latest.after;$evidence.capture_origin=$latest.capture_origin;$evidence.regions=$latest.regions;$evidence.region_analysis=@();$evidence.deferred_caret=($null -eq $caret)
                if(!$caret){Unverified 'IME text gates passed; candidate selection awaits the actual caret in the gracefully flushed trace' $evidence}
                Assess-Candidate $evidence $caret
                Unverified ('IME text gates passed; masked candidate detection requires visual review: '+$evidence.selection_status) $evidence
            }finally{[PocFunctionalNative]::ImeOpen($window,[uint32]$review.Id,$false)}
        }
        Check 'F5' 'UI Automation identifiers and patterns' {
            Set-Filter '';Props-Scroll 0;$patterns=[Collections.Generic.List[object]]::new()
            Shared-PatternCheck 'drawer-toggle Invoke' {$drawer=Control 'drawer';$before=Bounds-Array $drawer;$offscreen=$drawer.Current.IsOffscreen;(Pattern (Control 'drawer-toggle') ([System.Windows.Automation.InvokePattern])).Invoke();Wait-Until {$now=Bounds-Array $drawer;$drawer.Current.IsOffscreen -ne $offscreen -or $now[0] -ne $before[0] -or $now[2] -ne $before[2]} 'Drawer Invoke produced no observed UIA geometry change';@{before=$before;after=(Bounds-Array $drawer);offscreen_before=$offscreen;offscreen_after=$drawer.Current.IsOffscreen}} $patterns
            foreach($id in $spec.ids){Shared-PatternCheck ("id:$id") {$control=Control $id;@{id=$control.Current.AutomationId;type=$control.Current.ControlType.ProgrammaticName}} $patterns}
            Shared-PatternCheck 'slider-size RangeValue 42' {$slider=Control 'slider-size';$value=Pattern $slider ([System.Windows.Automation.RangeValuePattern]);$value.SetValue(42);Wait-Until {[Math]::Abs($value.Current.Value-42) -lt .01} 'Slider did not acknowledge RangeValue42';$value.Current.Value} $patterns
            Shared-PatternCheck 'text-field Value abc' {Set-Filter 'abc';(Pattern (Control 'text-field') ([System.Windows.Automation.ValuePattern])).Current.Value} $patterns
            Set-Filter ''
            $virtualSteps=[Collections.Generic.List[object]]::new()
            try{$row=if($which -eq 'winui'){Virtual-Row 500 $virtualSteps}else{Row 500};$patterns.Add(@{pattern='layer-row-00500 ScrollItem';status='pass';steps=@($virtualSteps);value=@{id=$row.Current.AutomationId;bounds=(Bounds-Array $row)}})}catch{if($virtualSteps.Count){$virtualSteps[-1].status='fail';$virtualSteps[-1].error=$_.Exception.Message};$patterns.Add(@{pattern='layer-row-00500 ScrollItem';status='fail';steps=@($virtualSteps);error=$_.Exception.Message})}
            Shared-PatternCheck 'menu-button Invoke' {(Pattern (Menu-Control) ([System.Windows.Automation.InvokePattern])).Invoke();$true} $patterns
            foreach($id in $spec.menu_ids){Shared-PatternCheck ("menu:$id") {$item=@(Popup-Items|Where-Object {$_.Current.AutomationId -eq $id})[0];Assert ($null -ne $item) "Missing menu ID $id";$invoke=Pattern $item ([System.Windows.Automation.InvokePattern]);@{id=$item.Current.AutomationId;invoke_available=($null -ne $invoke)}} $patterns}
            Key 27;$failed=@($patterns|Where-Object status -eq 'fail')
            @{status=$(if($failed.Count){'fail'}else{'pass'});patterns=@($patterns)}
        }
        Check 'F6' 'DPI, exact marker cells and probe pixels' {
            Set-Filter '';Props-Scroll 0;$scale=[PocFunctionalNative]::GetDpiForWindow($window)/96.0;$expected=if($Vm -eq 'capycanvas6-poc-gpu'){1.25}else{1.5};Assert ([Math]::Abs($scale-$expected) -lt .001) "Effective scale $scale differs from required $expected"
            $shot=Snapshot 'F6-client';$client=[PocFunctionalNative+Rect]::new();[PocFunctionalNative]::GetClientRect($window,[ref]$client)|Out-Null
            Assert ([PocFunctionalPixels]::Marker($shot.path,4,44)) 'UI marker is not framed physical4x4 cells'
            Assert ([PocFunctionalPixels]::Marker($shot.path,4,$client.Height-12)) 'Canvas marker is not framed physical4x4 cells'
            $groupWidth=($spec.metrics.column_left-$spec.metrics.column_right)*$scale;$left=$client.Width-$spec.metrics.column_left*$scale;$measurements=@{}
            foreach($entry in @(@('divider',(1/6.0),2),@('glass',.5,2),@('fill',(5/6.0),1))){$color=$spec.probes.($entry[0]);$probeX=if($entry[0] -eq 'glass'){[int]($left+$groupWidth+$spec.probe_metrics.glass_gutter_x)}else{[int]($left+$entry[1]*$groupWidth)};$runs=[PocFunctionalPixels]::ColorRuns($shot.path,$probeX,$color[0],$color[1],$color[2]);Assert ($runs.Length -gt 0) "No exact $($entry[0]) probe pixels found";for($i=1;$i -lt $runs.Length;$i+=2){Assert ($runs[$i] -eq $entry[2]) "$($entry[0]) probe is $($runs[$i]) physical px instead of $($entry[2])"};$measurements[$entry[0]]=$runs}
            $title=(Control 'title-bar').Current.BoundingRectangle;$row=(Row 0).Current.BoundingRectangle;Assert ([Math]::Abs($title.Height-$spec.metrics.title_height*$scale) -le 1) 'Title bar does not use the effective scale';Assert ([Math]::Abs($row.Height-$spec.metrics.row_height*$scale) -le 1) 'List row does not use the effective scale'
            Unverified 'Pixel dimensions pass; text raster quality requires review of the actual crop. Mixed-DPI moves require a second monitor' @{scale=$scale;marker_cells_physical_px=4;probes=$measurements;title_height=$title.Height;row_height=$row.Height;crop=$shot.path;geometry='pass';text_raster_review='unverified';mixed_dpi='unverified'}
        }
        Check 'F7' 'Title bar, maximize, keyboard and caption drag' {
            Foreground;$caption=Control 'title-bar';$bounds=Maximize-Bounds;if(!$bounds){Unverified 'UIA and WM_GETTITLEBARINFOEX exposed no actual maximize-button bounds'}
            $x=[int]($bounds[0]+$bounds[2]/2);$y=[int]($bounds[1]+$bounds[3]/2);$hit=[PocFunctionalNative]::HitTest($window,[uint32]$review.Id,$x,$y);Assert ($hit -eq 9) "Maximize button WM_NCHITTEST returned $hit instead of HTMAXBUTTON9"
            Tap $bounds;Wait-Until {[PocFunctionalNative]::Maximized($window,[uint32]$review.Id)} 'Click maximize did not maximize';$restore=Maximize-Bounds;Assert ($null -ne $restore) 'Restore button bounds unavailable';Tap $restore;Wait-Until {![PocFunctionalNative]::Maximized($window,[uint32]$review.Id)} 'Caption restore did not restore'
            $before=[PocFunctionalNative+Rect]::new();[PocFunctionalNative]::GetWindowRect($window,[ref]$before)|Out-Null;$bar=$caption.Current.BoundingRectangle;$drag=$null
            for($n=2;$n -lt 14;$n++){$px=[int]($bar.Left+$bar.Width*$n/16);$py=[int]($bar.Top+$bar.Height/2);if([PocFunctionalNative]::HitTest($window,[uint32]$review.Id,$px,$py) -eq 2){$drag=@($px,$py);break}}
            Assert ($null -ne $drag) 'No actual HTCAPTION point exists in empty title-bar space';[CapyRowPointer]::Down('mouse',$drag[0],$drag[1]);try{[CapyRowPointer]::Move($drag[0]+80,$drag[1]+40)}finally{[CapyRowPointer]::Up()};$after=[PocFunctionalNative+Rect]::new();Wait-Until {[PocFunctionalNative]::GetWindowRect($window,[ref]$after)|Out-Null;$after.Left -ne $before.Left -or $after.Top -ne $before.Top} 'Caption drag did not move the window'
            @{status='unverified';native_maximize_restore='pass';caption_drag='pass';maximize_bounds=$bounds;hit_test=$hit;caption_before=$before;caption_after=$after;win_up_down=@{status='unverified';reason='User accepted private-desktop session limit after the WinUI Win+Up failure';applies_to='both arms'}}
        }
        Check 'F9' 'Private clipboard text round trip' {
            Props-Scroll 60;$entry=Control 'text-field';$expected='Capy clipboard 日本語';(Pattern $entry ([System.Windows.Automation.ValuePattern])).SetValue($expected);Focus $entry;Chord ([uint16[]]@(17)) 65;Chord ([uint16[]]@(17)) 67
            Assert ([System.Windows.Forms.Clipboard]::GetText() -ceq $expected) 'Ctrl+C did not put the selected text on the private test clipboard'
            (Pattern $entry ([System.Windows.Automation.ValuePattern])).SetValue('');Chord ([uint16[]]@(17)) 86;Wait-Until {(Pattern $entry ([System.Windows.Automation.ValuePattern])).Current.Value -ceq $expected} 'Ctrl+V did not restore the copied text';Set-Filter ''
            @{status='pass';text=$expected;clipboard_format='CF_UNICODETEXT'}
        }
        Check 'F10' 'Popup extends outside the window while on screen' {
            Foreground;$geometry=Place-Ui 800 600 80 80;$geometry|ConvertTo-Json -Depth 4|Set-Content -LiteralPath (Join-Path $armOut 'F10-placement.json') -Encoding UTF8;(Pattern (Menu-Control) ([System.Windows.Automation.InvokePattern])).Invoke();$items=@(Popup-Items)
            $outer=[PocFunctionalNative+Rect]::new();[PocFunctionalNative]::GetWindowRect($window,[ref]$outer)|Out-Null;$shot=Snapshot 'F10-popup-screen' -Screen;$origin=@($shot.metadata.origin);$screenRight=$origin[0]+$shot.metadata.width;$screenBottom=$origin[1]+$shot.metadata.height
            foreach($item in $items){Assert (!$item.Current.IsOffscreen) 'A popup item is off screen';Assert (![string]::IsNullOrEmpty($item.Current.Name)) 'A popup item has no rendered label'}
            $container=Popup-Container $items
            if(!$container){Key 27;Unverified 'No owned UIA popup container or unique native retained popup HWND exposes the actual popup rectangle; item bounds do not include its padding' @{window=$outer;screen_capture=$shot.path;item_bounds=@($items|ForEach-Object {@{id=$_.Current.AutomationId;bounds=(Bounds-Array $_)}})}}
            $bounds=$container.bounds;$left=$bounds[0];$top=$bounds[1];$right=$left+$bounds[2];$bottom=$top+$bounds[3]
            Assert ($left -lt $outer.Left -or $top -lt $outer.Top -or $right -gt $outer.Right -or $bottom -gt $outer.Bottom) 'The actual MenuFlyout stayed entirely inside the main window'
            Assert ($left -ge $origin[0] -and $top -ge $origin[1] -and $right -le $screenRight -and $bottom -le $screenBottom) 'The actual popup extends off screen'
            $crop=Join-Path $armOut 'F10-popup.png';[PocFunctionalPixels]::Crop($shot.path,$crop,[int]($left-$origin[0]),[int]($top-$origin[1]),[int]($right-$left),[int]($bottom-$top));Assert ([PocFunctionalPixels]::Colors($crop) -ge 3) 'Popup crop has no visible item content';Key 27
            @{status='pass';popup=@($left,$top,($right-$left),($bottom-$top));container=$container;window=$outer;crop=$crop;items=@($items|ForEach-Object {$_.Current.AutomationId})}
        }
    }catch{
        $harnessErrors.Add(@{arm=$which;stage='UI application';error=$_.Exception.Message;repro=$repro})
        foreach($id in @('F3','F4','F5','F6','F7','F9','F10')){if(!@($results|Where-Object {$_.arm -eq $which -and $_.id -eq $id}).Count){Check $id 'UI application prerequisite' {Unverified 'The owned UI application could not be prepared; see harness_errors'}}}
    }finally{try{Close-Ui}catch{Save-Results}}
    try{Resolve-DeferredText}catch{$harnessErrors.Add(@{arm=$which;stage='deferred text evidence';error=$_.Exception.Message;repro=$repro});Save-Results}
    try{Resolve-DeferredCaret}catch{$harnessErrors.Add(@{arm=$which;stage='deferred caret evidence';error=$_.Exception.Message;repro=$repro});Save-Results}
    Check 'F8' 'Touch inertia' {
        $contacts=@(Observed 'scroll-touch');$durations=[Collections.Generic.List[object]]::new()
        foreach($contact in $contacts){$meta=Get-Content -LiteralPath (Join-Path $contact 'meta.json') -Raw|ConvertFrom-Json;$inputs=@(Json-Lines (Join-Path $contact 'inputs.jsonl')|Where-Object {!$_.warmup});$frames=@(Json-Lines (Join-Path $contact 'frames.jsonl')|Where-Object {$_.last_present_qpc -gt 0 -and $null -ne $_.scroll_offset});foreach($release in @($inputs|Where-Object kind -eq 'touch-up')){$next=@($inputs|Where-Object {$_.kind -eq 'touch-down' -and $_.qpc -gt $release.qpc}|Select-Object -First 1);$limit=if($next.Count){$next[0].qpc}else{($frames|Select-Object -Last 1).last_present_qpc};$previous=$null;$last=$null;foreach($frame in $frames|Where-Object {$_.last_present_qpc -ge $release.qpc -and $_.last_present_qpc -lt $limit}){if($null -ne $previous -and $frame.scroll_offset -ne $previous){$last=$frame.last_present_qpc};$previous=$frame.scroll_offset};$ms=if($null -ne $last){1000.0*($last-$release.qpc)/$meta.qpc_frequency}else{0};$durations.Add(@{release_qpc=$release.qpc;inertia_ms=$ms;contact=$contact})}}
        if(!$durations.Count){Unverified 'No captured touch release/scroll-ruler sequence was available'}
        @{status=$(if(@($durations|Where-Object {$_.inertia_ms -lt 200}).Count){'fail'}else{'pass'});flings=@($durations);required_minimum_ms=200}
    }
    Check 'F11' 'Displayed content, glass and fill drift' {
        $distances=[Collections.Generic.List[double]]::new();$contacts=@(Observed 'divider')
        foreach($contact in $contacts){$meta=Get-Content -LiteralPath (Join-Path $contact 'meta.json') -Raw|ConvertFrom-Json;foreach($frame in Json-Lines (Join-Path $contact 'frames.jsonl')){if($frame.last_present_qpc -ge $meta.motion_start_qpc -and $frame.last_present_qpc -le $meta.motion_end_qpc -and $null -ne $frame.divider_y -and $null -ne $frame.glass_y -and $null -ne $frame.fill_y){$values=@([double]$frame.divider_y,[double]$frame.glass_y,[double]$frame.fill_y);$distances.Add(($values|Measure-Object -Maximum).Maximum-($values|Measure-Object -Minimum).Minimum)}}}
        if(!$distances.Count){Unverified 'No desktop frame contained all three independently produced edge probes'}
        @{status='pass';measurement_only=$true;frames=$distances.Count;max_px=($distances|Measure-Object -Maximum).Maximum;share_over_1px=@($distances|Where-Object {$_ -gt 1}).Count/[double]$distances.Count;evidence=$contacts}
    }
    Check 'F12' 'List layout and recording at 1,000 versus 10,000 rows' {
        $measurements=@{}
        foreach($rows in @(1000,10000)){
            try{
                $contacts=@(Observed 'scroll-wheel' $rows);$layout=0.0;$record=0.0;$steps=0;$timed=0
                foreach($contact in $contacts){$meta=Get-Content -LiteralPath (Join-Path $contact 'meta.json') -Raw|ConvertFrom-Json;$steps+=@(Json-Lines (Join-Path $contact 'inputs.jsonl')|Where-Object {$_.kind -eq 'wheel' -and !$_.warmup -and !$_.terminal}).Count;foreach($event in Json-Lines (Join-Path $contact 'canvas.jsonl')){if($event.qpc -ge $meta.motion_start_qpc -and $event.qpc -le $meta.motion_end_qpc -and ($event.event -match 'scroll' -or $event.cause -match 'scroll') -and $null -ne $event.layout_ms -and $null -ne $event.record_ms){$layout+=[double]$event.layout_ms;$record+=[double]$event.record_ms;$timed++}}}
                $measurements[[string]$rows]=@{layout_ms=$layout;record_ms=$record;accepted_scroll_steps=$steps;actual_timing_records=$timed;contacts=$contacts}
                if($timed -and $steps){$measurements[[string]$rows].layout_per_step=$layout/$steps;$measurements[[string]$rows].record_per_step=$record/$steps}
            }catch{$measurements[[string]$rows]=@{status='fail';error=$_.Exception.Message}}
        }
        if(@(1000,10000|Where-Object {$measurements[[string]$_].status -eq 'fail'}).Count){return @{status='fail';measurements=$measurements}}
        if(@(1000,10000|Where-Object {!$measurements[[string]$_].actual_timing_records -or !$measurements[[string]$_].accepted_scroll_steps}).Count){Unverified "The $which traces do not expose actual scroll layout/record timings for both list sizes; framework callback duration is not a substitute" $measurements}
        $comparison=@{}
        foreach($metric in @('layout_per_step','record_per_step')){$a=$measurements['1000'][$metric];$b=$measurements['10000'][$metric];$comparison[$metric]=@{within_10_percent=$(if($a -eq 0){$b -eq 0}else{[Math]::Abs($b-$a)/$a -le .1});relative_difference=$(if($a -eq 0){$null}else{[Math]::Abs($b-$a)/$a})}}
        @{status=$(if(@($comparison.Values|Where-Object {!$_.within_10_percent}).Count){'fail'}else{'pass'});measurements=$measurements;comparison=$comparison;uia_10000_rows='not used'}
    }
    Save-Results
}
Save-Results
if(@($results|Where-Object status -eq 'fail').Count -or $harnessErrors.Count){throw "Functional checks recorded failures; see $(Join-Path $Out 'functional.json')"}
