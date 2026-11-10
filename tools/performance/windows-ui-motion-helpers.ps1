if(!('CapyRowPointer' -as [type])){Add-Type -Path (Join-Path $repo 'apps/layer-windows/scripts/RowPointerDriver.cs')}
if(!('CapyMotionNative' -as [type])){Add-Type -TypeDefinition @'
using System;using System.Runtime.InteropServices;
public static class CapyMotionNative {
 [DllImport("user32.dll")]public static extern bool SetWindowPos(IntPtr h,IntPtr after,int x,int y,int w,int height,uint flags);
 [DllImport("user32.dll")]public static extern IntPtr SendMessage(IntPtr h,uint m,IntPtr w,IntPtr l);
}
'@}
function Motion-Box([string]$Id){(Control $Id -Arranged).Current.BoundingRectangle}
function Motion-SettledBox([string]$Id){
 $settled=@{box=$null;count=0}
 Wait-Until {
  $box=Motion-Box $Id
  if($box -eq $settled.box){$settled.count++}else{$settled.box=$box;$settled.count=0}
  $settled.count -ge 3
 } "Native bounds did not settle: $Id"
 $settled.box
}
function Motion-VerifyBundle([string]$Record){
 $bundle=Get-Content -LiteralPath $Record -Raw|ConvertFrom-Json
 if(!$bundle.source_sha -or !$bundle.files.Count){throw 'Frozen build manifest is incomplete'}
 foreach($file in $bundle.files){
  $path=[IO.Path]::GetFullPath((Join-Path $bundle.root $file.path))
  if(!$path.StartsWith($bundle.root.TrimEnd('\')+'\',[StringComparison]::OrdinalIgnoreCase)){throw 'Manifest path escaped frozen build'}
  if((Get-FileHash -LiteralPath $path).Hash -ne $file.sha256){throw "Frozen binary changed: $path"}
 }
}
function Motion-VerifySeed([string]$Directory){
 $names=[Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
 $manifest=Get-Content -LiteralPath (Join-Path $Directory 'hashes.json') -Raw|ConvertFrom-Json
 foreach($entry in $manifest){
  $name=[IO.Path]::GetFileName($entry.Path)
  if(!$name -or !$entry.Hash -or !$names.Add($name)){throw 'Frozen seed manifest is incomplete or ambiguous'}
  $path=Join-Path $Directory $name
  if((Get-FileHash -LiteralPath $path).Hash -ne $entry.Hash){throw "Frozen seed changed: $name"}
 }
 foreach($name in @('seed.json','blank.capy','layers-50.capy','setup-model.json')){if(!$names.Contains($name)){throw "Frozen seed manifest omits $name"}}
}
function Motion-Pen([string]$Project){
 $monitorPlan=Join-Path $run 'pen-monitor-plan.json';$monitorOut=Join-Path $run 'pen-cpu';$stop=Join-Path $run 'pen-monitor-stop'
 @{executable=$Executable;registry_directory=(Split-Path $Executable);stop=$stop;profile=[bool]$ProfileDirectory}|ConvertTo-Json|Set-Content -LiteralPath $monitorPlan
 $monitor=Start-Process -FilePath $Observer -ArgumentList @('--monitor',('"'+$monitorPlan+'"'),('"'+$monitorOut+'"')) -PassThru -RedirectStandardError (Join-Path $run 'monitor-stderr.log')
 $null=$monitor.Handle
 try{
  Wait-Until {(Test-Path -LiteralPath (Join-Path $monitorOut 'ready.json')) -or $monitor.HasExited} 'External pen CPU monitor did not start'
  if($monitor.HasExited){throw 'External pen CPU monitor exited before pen launch'}
  & (Join-Path $PSScriptRoot 'windows-pen-latency.ps1') -Executable $Executable -Project $Project -OutputDirectory (Join-Path $run 'pen') -BrushPreset Pencil -Seconds 6 -SkipPresentMon
 }finally{
  [IO.File]::WriteAllText($stop,'stop')
  if(!$monitor.WaitForExit(10000)){throw 'Owned pen CPU monitor did not exit'}
 }
 if($monitor.ExitCode -ne 0){throw 'External pen CPU monitor failed'}
}
function Motion-ProfileStart{
 if(!$ProfileDirectory){return}
 [IO.Directory]::CreateDirectory($ProfileDirectory)|Out-Null
 [IO.File]::WriteAllText((Join-Path $ProfileDirectory 'ready.json'),'{"ready":true}')
 Wait-Until {Test-Path -LiteralPath (Join-Path $ProfileDirectory 'go.json')} 'Elevated profiler did not start'
}
function Motion-ProfileStop{
 if(!$ProfileDirectory){return}
 [IO.File]::WriteAllText((Join-Path $ProfileDirectory 'finished.json'),'{"finished":true}')
 Wait-Until {Test-Path -LiteralPath (Join-Path $ProfileDirectory 'stopped.json')} 'Elevated profiler did not stop'
}
function Motion-Observe([string]$Plan,[string]$Output){
 $name=Split-Path -Leaf $Output
 $process=Start-Process -FilePath $Observer -ArgumentList @(('"'+$Plan+'"'),('"'+$Output+'"')) -PassThru -NoNewWindow -RedirectStandardOutput (Join-Path $run ($name+'-observer.log')) -RedirectStandardError (Join-Path $run ($name+'-observer-stderr.log'))
 $null=$process.Handle;$process.WaitForExit()
 if($process.ExitCode -ne 0){throw "Observer $name failed: $([IO.File]::ReadAllText((Join-Path $run ($name+'-observer-stderr.log'))))"}
}
function Motion-Centre($Box){@([int]($Box.Left+$Box.Width/2),[int]($Box.Top+$Box.Height/2))}
function Motion-SelectPanel([string]$Panel){
 $tab=Find ('panel-tab-'+$Panel) -Visible
 if($tab){$tab.GetCurrentPattern([Windows.Automation.InvokePattern]::Pattern).Invoke()}
}
function Motion-Key([uint16]$Key,[uint16[]]$Modifiers=@()){
 [CapyRowPointer]::Chord([uint32]$review.Id,$Modifiers,$Key)
}
function Motion-ProviderClosed([Exception]$Exception){$Exception.GetBaseException().HResult -in @(-2147418094,-2147417848)}
function Motion-Close{
 if(!$review -or $review.HasExited){return}
 [CapyWindowApi]::PostMessage($drawingWindow,0x10,[UIntPtr]::Zero,[IntPtr]::Zero)|Out-Null
 $discardCondition=[Windows.Automation.AndCondition]::new(
  [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::ProcessIdProperty,[int]$review.Id),
  [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::NameProperty,'Discard Changes'),
  [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::ControlTypeProperty,[Windows.Automation.ControlType]::Button))
 $decision=@{clicked=$false;providerClosed=$false}
 Wait-Until {
  if($review.HasExited){return $true}
  if(!$decision.providerClosed){
   try{
    $discard=[Windows.Automation.AutomationElement]::RootElement.FindFirst([Windows.Automation.TreeScope]::Descendants,$discardCondition)
    if($discard -and !$decision.clicked){$discard.GetCurrentPattern([Windows.Automation.InvokePattern]::Pattern).Invoke();$decision.clicked=$true}
   }catch{if(!(Motion-ProviderClosed $_.Exception)){throw};$decision.providerClosed=$true}
  }
  $review.Refresh();$review.HasExited
 } 'Owned benchmark app did not close' 30 -Closing
}
function Motion-Restore([string]$Name,$Seed){
 [CapyWindowApi]::SetForegroundWindow($drawingWindow)|Out-Null
 if($Name -in @('resize-native','resize-programmatic')){[CapyMotionNative]::SetWindowPos($drawingWindow,[IntPtr]::Zero,48,48,2050,1400,0x14)|Out-Null}
 if($Name -eq 'drawer' -and (Find 'tool-drawer' -Visible)){Invoke-Id $Seed.color_id}
 if($Name -eq 'expansion' -and (Find 'panel-configuration-sizes' -Visible)){Motion-Key 0x1b}
 if($Name -eq 'typing'){
  if(!(Find 'command-search' -Visible)){Motion-Key 0x4b @(0x11)}
  (Control 'command-search').GetCurrentPattern([Windows.Automation.ValuePattern]::Pattern).SetValue('');(Control 'command-search').SetFocus()
 }
 if($Name -eq 'slider'){
  (Control 'tool-setting-size-slider').GetCurrentPattern([Windows.Automation.RangeValuePattern]::Pattern).SetValue(.5)
 }
 if($Name -eq 'wheel'){
  $scroll=@{pattern=$null}
  Wait-Until {$scroll.pattern=Scroll-Pattern (Control 'layer-list' -Arranged);$scroll.pattern.Current.VerticallyScrollable} '50-layer list has no arranged overflow'
  $scroll.pattern.SetScrollPercent(-1,30)
 }
}
function Motion-Probe([string]$Name,[string]$Kind,[int[]]$Rect,[int]$Axis=0,[int]$Anchor=0){@{name=$Name;kind=$Kind;rect=$Rect;axis=$Axis;anchor=$Anchor}}
function Motion-Snapshot($Probe){
 [CapyWindowApi]::SetForegroundWindow($drawingWindow)|Out-Null
 $id='endpoint-'+[Guid]::NewGuid().ToString('N');$path=Join-Path $run ($id+'.json');$output=Join-Path $run $id
 @{process_id=$review.Id;hwnd=$drawingWindow.ToInt64();workload='observe';seconds=.1;scale=([CapyWindowApi]::GetDpiForWindow($drawingWindow)/96.);refresh_hz=120.;point=@(0,0);amplitude=0.;axis=0;window=@(0,0,0,0);probes=@($Probe);no_capture=$false}|ConvertTo-Json -Depth 12|Set-Content -LiteralPath $path
 Motion-Observe $path $output
 $frames=@(Get-Content -LiteralPath (Join-Path $output 'frames.jsonl')|ForEach-Object {$_|ConvertFrom-Json})
 if(!$frames.Count){throw 'No composed endpoint pixels'}
 $hash=$frames[-1].probes.($Probe.name).hash
 if(!$hash){throw 'Missing endpoint fingerprint'}
 $hash
}
function Motion-Plan([string]$Name,$Seed){
 if($Name -eq 'slider'){Motion-SelectPanel 'tool_settings'}
 if($Name -eq 'wheel'){Motion-SelectPanel 'layers'}
 if($Name -eq 'expansion'){Motion-SelectPanel 'sizes'}
 Motion-Restore $Name $Seed
 $scale=[CapyWindowApi]::GetDpiForWindow($drawingWindow)/96.
 $window=New-Object CapyWindowApi+Rect;[CapyWindowApi]::GetWindowRect($drawingWindow,[ref]$window)|Out-Null
 $outer=@($window.left,$window.top,($window.right-$window.left),($window.bottom-$window.top))
 $point=@(0,0);$axis=0;$amplitude=60*$scale;$menu=@(0,0);$probes=@()
 switch($Name){
  {$_ -in @('divider','column')}{
   $id=if($Name -eq 'divider'){'divider-13'}else{'divider-11'}
   $box=Motion-Box $id;$point=Motion-Centre $box;$axis=if($Name -eq 'divider'){1}else{0}
   if(($axis -eq 1 -and $box.Width -lt $box.Height) -or ($axis -eq 0 -and $box.Height -lt $box.Width)){throw 'Divider orientation differs from the calibrated Paint workspace'}
   $edge=$point[$axis];$rect=if($axis){@($point[0],($edge-100),3,400)}else{@(($edge-100),($point[1]-20),400,3)}
   $anchor=100
   if($Name -eq 'column'){
    $tab=Motion-Box 'panel-tab-navigator';$rect[1]=[int]($tab.Top+$tab.Height/2);$anchor=[int]($tab.Left+16)-$rect[0];$amplitude=30*$scale
   }
   $probes+=Motion-Probe 'edge' 'edge' $rect $axis $anchor
  }
  {$_ -in @('resize-native','resize-programmatic')}{
   $point=@(($window.right-2),[int]($window.top+$outer[3]*.6));$amplitude=250
   $packed=[IntPtr](([long]($point[1] -band 65535) -shl 16) -bor ($point[0] -band 65535))
   if($Name -eq 'resize-native' -and [CapyMotionNative]::SendMessage($drawingWindow,0x84,[IntPtr]::Zero,$packed).ToInt64() -ne 11){throw 'Right border does not report HTRIGHT'}
   $tab=Motion-SettledBox 'panel-tab-navigator';$start=[int]$tab.Left-280
   $probes+=Motion-Probe 'content' 'edge' @($start,[int]($tab.Top+$tab.Height/2),600,3) 0 296
   $probes+=Motion-Probe 'frame' 'edge' @(($window.right-280),($window.top+18),560,3) 0 270
   $probes+=Motion-Probe 'black' 'black-grid' @(($window.left+20),($window.top+50),($outer[2]-40),($outer[3]-70))
  }
  'slider'{
   $box=Motion-Box 'tool-setting-size-slider';$point=Motion-Centre $box;$amplitude=$box.Width*.4
   $probes+=Motion-Probe 'value' 'edge' @([int]$box.Left,($point[1]-1),[int]$box.Width,3) 0 ([int]($box.Width/2)-16)
   $caption=Motion-Box 'tool-setting-size';$probes+=Motion-Probe 'caption' 'change' @([int]$caption.Left,[int]$caption.Top,[int]$caption.Width,[int]$caption.Height)
  }
  'drawer'{
   $point=Motion-Centre (Motion-Box $Seed.color_id)
   Invoke-Id $Seed.color_id;$box=Motion-SettledBox 'tool-drawer'
   $probe=Motion-Probe 'drawer' 'change' @(([int]$box.Left-4),[int]($box.Top+$box.Height/2),([int]$box.Width+8),3)
   $openHash=Motion-Snapshot $probe;Invoke-Id $Seed.color_id
   Wait-Until {!(Find 'tool-drawer' -Visible)} 'Drawer did not close after calibration'
   $probe.endpoints=@((Motion-Snapshot $probe),$openHash)
   if($probe.endpoints[0] -eq $probe.endpoints[1]){throw 'Drawer probe does not distinguish open and closed pixels'}
   $probes+=$probe
  }
  'expansion'{
   $tab=Control 'panel-tab-sizes' -Arranged;$point=Motion-Centre $tab.Current.BoundingRectangle;$tab.SetFocus();Motion-Key 0x79 @(0x10)
   $entry=Control $Seed.configuration_label -Name -Type ([Windows.Automation.ControlType]::MenuItem) -Arranged;$menu=Motion-Centre $entry.Current.BoundingRectangle
   $entry.GetCurrentPattern([Windows.Automation.InvokePattern]::Pattern).Invoke();$box=Motion-SettledBox 'panel-configuration-sizes'
   $probe=Motion-Probe 'configuration' 'change' @(([int]$box.Left-4),[int]($box.Top+$box.Height/2),([int]$box.Width+8),3)
   $openHash=Motion-Snapshot $probe
   Motion-Key 0x1b;Wait-Until {!(Find 'panel-configuration-sizes' -Visible)} 'Configuration did not close after calibration'
   $probe.endpoints=@((Motion-Snapshot $probe),$openHash)
   if($probe.endpoints[0] -eq $probe.endpoints[1]){throw 'Expansion probe does not distinguish open and closed pixels'}
   $probes+=$probe
  }
  'wheel'{
   $list=Control 'layer-list' -Arranged;$box=$list.Current.BoundingRectangle;$point=Motion-Centre $box;$axis=1
   $text=[Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::ControlTypeProperty,[Windows.Automation.ControlType]::Text)
   $names=@($list.FindAll([Windows.Automation.TreeScope]::Descendants,$text)|Where-Object {$_.Current.AutomationId -match '^layer-\d+-label$' -and !$_.Current.IsOffscreen -and $_.Current.BoundingRectangle.Width -ge 3})
   if(!$names.Count){throw 'No arranged layer names for scroll observation'}
   $label=$names[0].Current.BoundingRectangle
   $probes+=Motion-Probe 'scroll' 'shift' @([int]$label.Left,[int]($box.Top+20),[int][Math]::Min(100,$label.Width),[int][Math]::Min(500,$box.Height-40)) 1
  }
  'typing'{
   $box=Motion-Box 'command-search';$point=Motion-Centre $box
   $probes+=Motion-Probe 'field' 'change' @([int]$box.Left,[int]$box.Top,[int]$box.Width,[int]$box.Height)
   $box=Motion-Box 'command-results';$probe=Motion-Probe 'results' 'change' @([int]$box.Left,[int]$box.Top,[int]$box.Width,[int][Math]::Min(300,$box.Height))
   $fingerprints=@();$entry=Control 'command-search';$pattern=$entry.GetCurrentPattern([Windows.Automation.ValuePattern]::Pattern)
   foreach($length in 0..6){$pattern.SetValue('pencil'.Substring(0,$length));$fingerprints+=Motion-Snapshot $probe}
   $probe.endpoints=$fingerprints;$probes+=$probe;$pattern.SetValue('');$entry.SetFocus()
  }
 }
 if(!$probes.Count){throw "No probes for $Name"}
 @{process_id=$review.Id;hwnd=$drawingWindow.ToInt64();workload=$Name;seconds=6.;scale=$scale;refresh_hz=120.;point=$point;amplitude=$amplitude;axis=$axis;window=$outer;menu=$menu;probes=$probes;no_capture=$false}
}
