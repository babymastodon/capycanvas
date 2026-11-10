param(
 [Parameter(Mandatory)][string]$Executable,
 [Parameter(Mandatory)][string]$Observer,
 [Parameter(Mandatory)][string]$OutputDirectory,
 [Parameter(Mandatory)][string]$SeedDirectory,
 [string]$BuildRecord,
 [string]$ProfileDirectory,
 [ValidateSet('divider','column','resize-native','resize-programmatic','drawer','expansion','slider','wheel','typing','pen')][string]$Workload='divider',
 [ValidateSet('dark','light')][string]$Theme='dark',
 [ValidateRange(1,3)][int]$Contacts=3,
 [switch]$Preflight,
 [switch]$NoCapture
)
$ErrorActionPreference='Stop'
$repo=(Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
if(!$env:CAPY_PRIVATE_DESKTOP){
 & (Join-Path $repo 'apps/layer-windows/scripts/test-desktop.ps1') -Script $PSCommandPath -Parameters $PSBoundParameters -TimeoutSeconds 1800
 return
}
. (Join-Path $repo 'apps/layer-windows/scripts/CapyUia.ps1')
. (Join-Path $PSScriptRoot 'windows-ui-motion-helpers.ps1')
$Executable=(Resolve-Path -LiteralPath $Executable).Path
$Observer=(Resolve-Path -LiteralPath $Observer).Path
if(!$BuildRecord){$BuildRecord=Join-Path (Split-Path $Executable) 'build.json'}
Motion-VerifyBundle $BuildRecord
$OutputDirectory=[IO.Path]::GetFullPath($OutputDirectory)
$SeedDirectory=[IO.Path]::GetFullPath($SeedDirectory)
if(Test-Path -LiteralPath $OutputDirectory){throw 'Use a fresh benchmark output directory'}
[IO.Directory]::CreateDirectory($OutputDirectory)|Out-Null
$run=$OutputDirectory;$directory=$run;$CapyTraceDirectory=$run;$CapyWaitSeconds=60;$CapyPopups=$true
$review=$null;$drawingWindow=[IntPtr]::Zero
if($Workload -eq 'pen' -and $Theme -ne 'dark'){throw 'The existing pen series supports dark theme only'}
try{
 Enter-CapyEnvironment @('CAPY_LATENCY_TRACE')
 $env:CAPY_STORAGE_DIR=Join-Path $run 'profile'
 if(!$Preflight){
  if(!(Test-Path -LiteralPath (Join-Path $SeedDirectory 'seed.json'))){throw 'Run preflight before timing'}
  Motion-VerifySeed $SeedDirectory
  Copy-Item -LiteralPath (Join-Path $SeedDirectory 'profile') -Destination $env:CAPY_STORAGE_DIR -Recurse
 }
 $env:CAPY_LATENCY_TRACE='1';$env:CAPY_TEST_DISPLAY='1';$env:CAPY_TEST_PRIMARY='1'
 if($Preflight){$env:CAPY_TRACE_UI='1'}
 [IO.File]::WriteAllText((Settings-File),(@{language=@{Explicit='en'};theme=$Theme}|ConvertTo-Json -Depth 4))
 $project=if($Workload -eq 'wheel'){Join-Path $SeedDirectory 'layers-50.capy'}else{Join-Path $SeedDirectory 'blank.capy'}
 if(!$Preflight -and $Workload -eq 'pen'){
  Motion-ProfileStart
  Motion-Pen $project
  Motion-ProfileStop
  @{ok=$true;workload='pen';theme=$Theme;preflight=$false}|ConvertTo-Json|Set-Content -LiteralPath (Join-Path $run 'completed.json')
  return
 }
 $launch=@{FilePath=$Executable;WorkingDirectory=$run;PassThru=$true;RedirectStandardError=(Join-Path $run 'stderr.log')}
 if(!$Preflight){$launch.ArgumentList=@(('"'+$project+'"'))}
 $review=Start-Process @launch;$null=$review.Handle
 $owned=@{window=$null}
 Wait-Until {$owned.window=Owned-DrawingWindow $review;$owned.window -and $(if($Preflight){(Model).brush_ready}else{$true})} 'Owned drawing did not become ready' 120
 $root=$owned.window.Root;$drawingWindow=$owned.window.Handle
 [CapyRowPointer]::SetThreadDpiAwarenessContext([IntPtr](-4))|Out-Null
 [CapyWindowApi]::ShowWindow($drawingWindow,9)|Out-Null
 [CapyMotionNative]::SetWindowPos($drawingWindow,[IntPtr]::Zero,48,48,2048,1400,0x14)|Out-Null
 [CapyWindowApi]::SetForegroundWindow($drawingWindow)|Out-Null
 Wait-Until {(Control 'drawing-canvas').Current.IsEnabled -and !(Find 'canvas-status' -Visible) -and (Trace-File 'presentation-probe') -and ($Preflight -or $root.Current.Name -eq ([IO.Path]::GetFileName($project)+' '+[char]0x00b7+' Capy Canvas'))} 'Requested drawing readiness did not settle' 120
 if([CapyWindowApi]::GetDpiForWindow($drawingWindow) -ne 120){throw 'Benchmark requires 125% scale'}
 if($Preflight){
  if(Test-Path -LiteralPath $SeedDirectory){throw 'Use a fresh preflight seed directory'}
  [IO.Directory]::CreateDirectory($SeedDirectory)|Out-Null
  Use-PaintWorkspace
  Expand-PanelColumn 'layers'
  Expand-PanelColumn 'tool_settings'
  $tile=@(((Model).panels|Where-Object id -eq 'toolbar').tiles|Where-Object {$_.control.kind -eq 'color'})
  if($tile.Count -ne 1){throw 'No unique toolbar color tile'}
  $configuration=@((Model).panels|Where-Object id -eq 'sizes')
  if($configuration.Count -ne 1){throw 'No unique Sizes configuration'}
  $seed=@{theme=$Theme;exe_sha256=(Get-FileHash -LiteralPath $Executable).Hash;dll_sha256=(Get-FileHash -LiteralPath (Join-Path (Split-Path $Executable) 'layer_windows.dll')).Hash;color_id=('tile-toolbar-'+$tile[0].id);configuration_label=($configuration[0].configuration_title+[char]0x2026);layer_count=50}
  $seed|ConvertTo-Json -Depth 5|Set-Content -Encoding UTF8 -LiteralPath (Join-Path $SeedDirectory 'seed.json')
  Save-ProjectAs (Join-Path $SeedDirectory 'blank.capy')
  Motion-SelectPanel 'layers'
  $count=@((Model).state.layers).Count
  if($count -gt 50){throw 'Preflight drawing already exceeds 50 layers'}
  for($i=$count;$i -lt 50;$i++){
   Invoke-Id 'layer-new'
   Wait-Until {@((Model).state.layers).Count -eq $i+1} '50-layer setup did not accept one new layer'
  }
  Save-ProjectAs (Join-Path $SeedDirectory 'layers-50.capy')
  if(@((Model).state.layers).Count -ne 50 -or @((Model).state.layers|Where-Object {$_.group -or !$_.visible}).Count){throw 'Expected exactly 50 visible ungrouped layers'}
  (Model)|ConvertTo-Json -Depth 100|Set-Content -LiteralPath (Join-Path $SeedDirectory 'setup-model.json')
  & (Join-Path $repo 'apps/layer-windows/scripts/exercise-window.ps1') -ProcessId $review.Id -WindowHandle $drawingWindow.ToInt64() -Action Close -StateDirectory $run
  Copy-Item -LiteralPath $env:CAPY_STORAGE_DIR -Destination (Join-Path $SeedDirectory 'profile') -Recurse
  $hashes=@(Get-ChildItem -LiteralPath $SeedDirectory -File|Get-FileHash|Select-Object Path,Hash)
  $hashes|ConvertTo-Json|Set-Content -LiteralPath (Join-Path $SeedDirectory 'hashes.json')
 }else{
  $seed=Get-Content -LiteralPath (Join-Path $SeedDirectory 'seed.json') -Raw|ConvertFrom-Json
  if($seed.exe_sha256 -ne (Get-FileHash -LiteralPath $Executable).Hash -or $seed.dll_sha256 -ne (Get-FileHash -LiteralPath (Join-Path (Split-Path $Executable) 'layer_windows.dll')).Hash){throw 'Seed belongs to different binaries; regenerate native calibration'}
  if(Trace-File 'ui-state'){throw 'Timing forbids CAPY_TRACE_UI'}
  $plan=Motion-Plan $Workload $seed
  $plan.seconds=1.;$plan.no_capture=$true
  $primePlan=Join-Path $run 'prime-plan.json';$plan|ConvertTo-Json -Depth 12|Set-Content -LiteralPath $primePlan
  Motion-Observe $primePlan (Join-Path $run 'prime')
  for($contact=0;$contact -lt $Contacts;$contact++){
   Motion-Restore $Workload $seed
   $plan=Motion-Plan $Workload $seed;$plan.seconds=6.;$plan.no_capture=[bool]$NoCapture
   $path=Join-Path $run ('plan-'+$contact+'.json');$plan|ConvertTo-Json -Depth 12|Set-Content -LiteralPath $path
   if($ProfileDirectory){
    $plan.profile=$true;$plan|ConvertTo-Json -Depth 12|Set-Content -LiteralPath $path
    Motion-ProfileStart
   }
   Motion-Observe $path (Join-Path $run ('contact-'+$contact))
   Motion-ProfileStop
   if($review.HasExited){throw 'App exited during measurement'}
  }
  Motion-Close
 }
 if($review -and !$review.HasExited){throw 'Owned app did not exit'}
 if($review -and $review.ExitCode -ne 0){throw "Owned app exit code $($review.ExitCode)"}
 if((Get-Item (Join-Path $run 'stderr.log')).Length){throw 'Owned app wrote stderr; inspect the preserved log'}
 @{ok=$true;workload=$Workload;theme=$Theme;preflight=[bool]$Preflight;source=(Get-FileHash -LiteralPath $PSCommandPath).Hash}|ConvertTo-Json|Set-Content -LiteralPath (Join-Path $run 'completed.json')
}catch{
 $_|Out-String|Set-Content -LiteralPath (Join-Path $run 'failure.txt')
 throw
}finally{
 try{if($review -and !$review.HasExited){Motion-Close}}finally{Exit-CapyEnvironment}
}
