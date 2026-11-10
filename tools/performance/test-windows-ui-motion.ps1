$ErrorActionPreference='Stop'
Add-Type -AssemblyName WindowsBase,UIAutomationClient,UIAutomationTypes
Add-Type -TypeDefinition @'
using System;
public static class CapyRowPointer {}
public static class CapyWindowApi {
 public struct Rect {public int left,top,right,bottom;}
 public static uint GetDpiForWindow(IntPtr h){return 120;}
 public static bool GetWindowRect(IntPtr h,out Rect r){r=new Rect{left=48,top=48,right=2098,bottom=1448};return true;}
 public static bool SetForegroundWindow(IntPtr h){return true;}
}
public static class CapyMotionNative {
 public static bool SetWindowPos(IntPtr h,IntPtr after,int x,int y,int w,int height,uint flags){return true;}
 public static IntPtr SendMessage(IntPtr h,uint m,IntPtr w,IntPtr l){return new IntPtr(11);}
}
'@
$repo=(Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
. (Join-Path $PSScriptRoot 'windows-ui-motion-helpers.ps1')
$seedPath=Join-Path ([IO.Path]::GetTempPath()) ('capy-motion-seed-'+[Guid]::NewGuid().ToString('N'))
try{
 [IO.Directory]::CreateDirectory($seedPath)|Out-Null
 foreach($name in @('seed.json','blank.capy','layers-50.capy','setup-model.json')){[IO.File]::WriteAllText((Join-Path $seedPath $name),$name)}
 $hashes=@(Get-ChildItem -LiteralPath $seedPath -File|Get-FileHash|Select-Object Path,Hash)
 $hashes|ConvertTo-Json|Set-Content -LiteralPath (Join-Path $seedPath 'hashes.json')
 Motion-VerifySeed $seedPath
 [IO.File]::AppendAllText((Join-Path $seedPath 'layers-50.capy'),'changed')
 $rejected=$false
 try{Motion-VerifySeed $seedPath}catch{$rejected=$_.Exception.Message -like 'Frozen seed changed:*'}
 if(!$rejected){throw 'A modified 50-layer drawing passed seed verification'}
}finally{if(Test-Path -LiteralPath $seedPath){Remove-Item -LiteralPath $seedPath -Recurse -Force}}
$closedProvider=[Runtime.InteropServices.COMException]::new('Closed provider',-2147418094)
if(!(Motion-ProviderClosed ([Exception]::new('Wrapped',$closedProvider)))){throw 'Terminal provider failure was not recognized'}
$disconnectedProvider=[Runtime.InteropServices.COMException]::new('Disconnected provider',-2147417848)
if(!(Motion-ProviderClosed ([Exception]::new('Wrapped',$disconnectedProvider)))){throw 'Disconnected closing provider was not recognized'}
if(Motion-ProviderClosed ([Runtime.InteropServices.COMException]::new('Access denied',-2147024891))){throw 'Unrelated provider failure was suppressed'}
$review=[pscustomobject]@{Id=42};$drawingWindow=[IntPtr]42
function Find {return $null}
function Motion-Box([string]$Id){
 switch($Id){
  'divider-13'{[Windows.Rect]::new(1400,500,300,4)}
  'divider-11'{[Windows.Rect]::new(1400,100,4,1000)}
  'panel-tab-navigator'{[Windows.Rect]::new(1404,110,180,25)}
  'tool-setting-size-slider'{[Windows.Rect]::new(100,100,300,30)}
  'tool-setting-size'{[Windows.Rect]::new(350,80,50,20)}
  default{throw "Unexpected native control: $Id"}
 }
}
function Control {
 $control=New-Object psobject
 $control|Add-Member ScriptMethod GetCurrentPattern {
  param($Pattern)
  $range=New-Object psobject;$range|Add-Member ScriptMethod SetValue {param($Value)};$range
 }
 $control
}
function Motion-SettledBox([string]$Id){Motion-Box $Id}
$cases=@(
 @{name='divider';rect='1550,402,3,400';axis=1},
 @{name='column';rect='1302,122,400,3';axis=0},
 @{name='resize-native';rect='1124,122,600,3';axis=0},
 @{name='resize-programmatic';rect='1124,122,600,3';axis=0},
 @{name='slider';rect='100,114,300,3';axis=0}
)
foreach($case in $cases){
 $plan=Motion-Plan $case.name @{}
 if(($plan.probes[0].rect -join ',') -ne $case.rect -or $plan.axis -ne $case.axis){throw "Wrong native probe geometry: $($case.name)"}
 if($plan.point.Count -ne 2 -or $plan.window.Count -ne 4){throw 'Malformed native input coordinates'}
 if($case.name -eq 'divider' -and $plan.probes[0].rect[$plan.axis]+$plan.probes[0].anchor -lt $plan.point[$plan.axis]){throw 'Divider template includes content above the moving tab strip'}
}
'Five window-free workload geometry cases passed'
