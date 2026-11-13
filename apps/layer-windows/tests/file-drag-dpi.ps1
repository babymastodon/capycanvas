param([string]$Output,[switch]$TypeSourceOnly)
$ErrorActionPreference='Stop'
$fixture=Join-Path $PSScriptRoot '../scripts/exercise-file-activation.ps1'
$tokens=$null;$errors=$null
$ast=[Management.Automation.Language.Parser]::ParseFile($fixture,[ref]$tokens,[ref]$errors)
if($errors.Count){throw 'File activation fixture did not parse'}
$definitions=@($ast.FindAll({param($node)
    ($node -is [Management.Automation.Language.StringConstantExpressionAst] -or $node -is [Management.Automation.Language.ExpandableStringExpressionAst]) -and $node.Value.Contains('public static class CapyFileDrag')
},$true)|ForEach-Object {$_.Value})
if($definitions.Count -ne 1){throw 'File activation drag type must have one definition'}


$drag=$definitions[0]
$send='[DllImport("user32.dll",SetLastError=true)] static extern uint SendInput(uint count,Input[] input,int size);'
if(!$drag.Contains($send)){throw 'File drag native input boundary declaration changed'}
$drag=$drag.Replace($send,'static uint SendInput(uint count,Input[] input,int size){if(count!=1)throw new Exception("Unexpected input packet count");return CapyFileDragDpiProbe.Observe(input[0].mouse.dx,input[0].mouse.dy,input[0].mouse.flags);}')
$probe=@'
public sealed class CapyFileDragDpiSample {
 public uint ThreadId;
 public long Context,ProcessContext;
 public int Awareness,ProcessAwareness,X,Y;
 public uint Flags;
 public int[] Metrics,PhysicalMetrics;
}
public static class CapyFileDragDpiProbe {
 [System.Runtime.InteropServices.DllImport("kernel32.dll")] public static extern uint GetCurrentThreadId();
 [System.Runtime.InteropServices.DllImport("kernel32.dll")] static extern System.IntPtr GetCurrentProcess();
 [System.Runtime.InteropServices.DllImport("user32.dll")] static extern System.IntPtr GetDpiAwarenessContextForProcess(System.IntPtr process);
 [System.Runtime.InteropServices.DllImport("user32.dll")] public static extern System.IntPtr GetThreadDpiAwarenessContext();
 [System.Runtime.InteropServices.DllImport("user32.dll")] static extern int GetAwarenessFromDpiAwarenessContext(System.IntPtr context);
 [System.Runtime.InteropServices.DllImport("user32.dll")] public static extern bool AreDpiAwarenessContextsEqual(System.IntPtr first,System.IntPtr second);
 [System.Runtime.InteropServices.DllImport("user32.dll")] public static extern System.IntPtr SetThreadDpiAwarenessContext(System.IntPtr context);
 [System.Runtime.InteropServices.DllImport("user32.dll")] static extern int GetSystemMetrics(int code);
 [System.Runtime.InteropServices.DllImport("user32.dll")] public static extern uint GetDpiForWindow(System.IntPtr window);
 [System.Runtime.InteropServices.DllImport("user32.dll")] public static extern uint GetDpiForSystem();
 static readonly System.Collections.Generic.List<CapyFileDragDpiSample> samples=new System.Collections.Generic.List<CapyFileDragDpiSample>();
 static int[] Metrics(){return new[]{GetSystemMetrics(76),GetSystemMetrics(77),GetSystemMetrics(78),GetSystemMetrics(79)};}
 public static CapyFileDragDpiSample Measure(){
  var context=GetThreadDpiAwarenessContext();var process=GetDpiAwarenessContextForProcess(GetCurrentProcess());var metrics=Metrics();int[] physical;
  var previous=SetThreadDpiAwarenessContext(new System.IntPtr(-4));
  if(previous==System.IntPtr.Zero)throw new System.ComponentModel.Win32Exception(System.Runtime.InteropServices.Marshal.GetLastWin32Error());
  try{physical=Metrics();}finally{if(SetThreadDpiAwarenessContext(previous)==System.IntPtr.Zero)throw new System.Exception("DPI sample did not restore its thread context");}
  return new CapyFileDragDpiSample{ThreadId=GetCurrentThreadId(),Context=context.ToInt64(),Awareness=GetAwarenessFromDpiAwarenessContext(context),ProcessContext=process.ToInt64(),ProcessAwareness=GetAwarenessFromDpiAwarenessContext(process),Metrics=metrics,PhysicalMetrics=physical};
 }
 public static uint Observe(int x,int y,uint flags){
  var sample=Measure();sample.X=x;sample.Y=y;sample.Flags=flags;lock(samples)samples.Add(sample);return 1;
 }
 public static void Reset(){lock(samples)samples.Clear();}
 public static CapyFileDragDpiSample[] Snapshot(){lock(samples)return samples.ToArray();}
}
'@
$deadlineProbe=@'
namespace CapyFileDropTest {
 public class UnavailableException:Exception{}
 public class Watch {public static Watch Current;public TimeSpan Elapsed;public static Watch StartNew(){return Current=new Watch();}}
 public static class State {
  public static int Dpi,Presses,Releases,Forms,Shows,Closes,Disposals,Joins,Events;public static string Effect,Error;public static bool Join=true;
  public static void Reset(){Dpi=Presses=Releases=Forms=Shows=Closes=Disposals=Joins=Events=0;Effect="Copy";Error=null;Join=true;Driver.Failure=null;}
 }
 public class Rect {public int X,Y,Width,Height;public Rect(int x,int y,int w,int h){X=x;Y=y;Width=w;Height=h;}}
 public class Glide {public bool Join(int ms){if(ms!=10000)throw new Exception("Source changed its glide join bound");State.Joins++;return State.Join;}}
 public static class Driver {
  public static Exception Failure;
  public static IntPtr SetThreadDpiAwarenessContext(IntPtr context){if(context.ToInt64()!=-4)throw new Exception("Source lost PMv2");State.Dpi++;return new IntPtr(-1);}
  public static Glide Press(IntPtr source,int x,int y){if(source.ToInt64()!=17||x!=140||y!=230)throw new Exception("Source changed its physical drag tuple");State.Presses++;return new Glide();}
  public static void Release(int x,int y){if(x!=140||y!=230)throw new Exception("Cleanup changed its physical release tuple");State.Releases++;}
 }
 public enum DragDropEffects {None,Copy}
 public static class DataFormats {public static string FileDrop="FileDrop";}
 public class DataObject {public string Format;public string[] Paths;public DataObject(string f,string[] p){Format=f;Paths=p;}}
 public static class Application {public static void DoEvents(){State.Events++;}}
 public class Form {
  public string FormBorderStyle,StartPosition;public bool ShowInTaskbar,TopMost;public Rect Bounds;public IntPtr Handle=new IntPtr(17);
  public Form(){if(State.Dpi==0)throw new Exception("Source Form preceded physical DPI context");State.Forms++;}
  public void Show(){if(FormBorderStyle!="None"||ShowInTaskbar||!TopMost||StartPosition!="Manual"||Bounds.X!=100||Bounds.Y!=390||Bounds.Width!=80||Bounds.Height!=80)throw new Exception("Source changed its Form geometry or presentation");State.Shows++;}
  public DragDropEffects DoDragDrop(DataObject data,DragDropEffects effect){if(data.Format!="FileDrop"||String.Join("|",data.Paths)!="photo 日本語.png|saved drawing.capy"||effect!=DragDropEffects.Copy||State.Presses!=1||State.Events!=2)throw new Exception("Source changed its native file-drop path");if(State.Error!=null)throw new Exception(State.Error);return (DragDropEffects)Enum.Parse(typeof(DragDropEffects),State.Effect);}
  public void Close(){State.Closes++;}public void Dispose(){State.Disposals++;}
 }
 public class Process {
  public bool HasExited;public int ExitCode,Kills,Waits,Disposals,HandleReads;public bool FailKill,FailWait;
  public IntPtr Handle {get{HandleReads++;return new IntPtr(91);}}
  public void Refresh(){}public void Kill(){Kills++;if(FailKill)throw new Exception("Cleanup kill failure");}
  public bool WaitForExit(int milliseconds){Waits++;if(milliseconds!=5000)throw new Exception("Cleanup changed its termination bound");if(FailWait)return false;HasExited=true;return true;}
  public void Dispose(){Disposals++;}
 }
 public class Runspace {public string ApartmentState;public void Open(){}public void Dispose(){}}
 public static class RunspaceFactory {public static Runspace CreateRunspace(){return new Runspace();}}
 public static class PowerShell {public static Shell Create(){return new Shell();}}
 public class Shell {
  public Runspace Runspace;public Shell AddScript(object script){return this;}public Shell AddArgument(object value){return this;}
  public object Invoke(){throw new Exception("Source drag invoked synchronously without a deadline");}public void Dispose(){}
 }
}
'@
$typeDefinition=$drag+"`n"+$probe+"`n"+$deadlineProbe
if($TypeSourceOnly){return $typeDefinition}
if(!$Output){throw 'File drag DPI test requires an output receipt path'}
Add-Type -AssemblyName UIAutomationClient,UIAutomationTypes,System.Drawing
if('CapyFileDrag' -as [type]){throw 'Run the real file drag DPI check in a fresh PowerShell process'}
Add-Type -TypeDefinition $typeDefinition
& {
 $actualDrop=$ast.Find({param($n)$n-is[Management.Automation.Language.FunctionDefinitionAst]-and$n.Name-eq'Drop-Files'},$true)
 if(!$actualDrop){throw 'Missing actual file-drop source journey'}
 $definition=$actualDrop.Extent.Text.Replace('[CapyWindowApi]','[CapyFileDropTest.Driver]').Replace('[CapyFileDrag]','[CapyFileDropTest.Driver]').Replace('[Windows.Forms.','[CapyFileDropTest.').Replace('[Drawing.Rectangle]','[CapyFileDropTest.Rect]').Replace('[powershell]','[CapyFileDropTest.PowerShell]').Replace('[runspacefactory]','[CapyFileDropTest.RunspaceFactory]')
 . ([scriptblock]::Create($definition))
 $fileDragDefinition=$definitions[0]
 $sourceAssignment=$actualDrop.Find({param($n)$n-is[Management.Automation.Language.AssignmentStatementAst]-and$n.Left.Extent.Text-eq'$source'},$true)
 $temp=Join-Path ([IO.Path]::GetTempPath()) ('capy-file-drop-'+[Guid]::NewGuid().ToString('N'));[IO.Directory]::CreateDirectory($temp)|Out-Null
 $savedScale=$env:CAPY_WAIT_SCALE
 try{
  if($sourceAssignment-and$sourceAssignment.Right.Extent.Text.StartsWith('{')){
   $sourceCode=$sourceAssignment.Right.Extent.Text.Replace('[CapyFileDrag]','[CapyFileDropTest.Driver]').Replace('[Windows.Forms.','[CapyFileDropTest.').Replace('[Drawing.Rectangle]','[CapyFileDropTest.Rect]')
   . ([scriptblock]::Create('$actualSource='+$sourceCode))
   $sourceReceipt=Join-Path $temp 'source.json'
   $sourcePayload=[Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes((@{paths=@('photo 日本語.png','saved drawing.capy');x=140;y=230;receipt=$sourceReceipt}|ConvertTo-Json -Compress)))
   function Add-Type($TypeDefinition,$AssemblyName){if($TypeDefinition-and$TypeDefinition-cne$fileDragDefinition){throw 'Child source changed its actual native drag definition'}}
   foreach($case in 'Copy','None','source-error','glide-error','join-error'){
    [CapyFileDropTest.State]::Reset()
    switch($case){'None'{[CapyFileDropTest.State]::Effect='None'}'source-error'{[CapyFileDropTest.State]::Error='Original OLE failure'}'glide-error'{[CapyFileDropTest.Driver]::Failure=[Exception]::new('Original glide failure')}'join-error'{[CapyFileDropTest.State]::Join=$false}}
    & $actualSource $fileDragDefinition $sourcePayload
    $result=[IO.File]::ReadAllText($sourceReceipt)|ConvertFrom-Json
    if($case-in'Copy','None'){if($result.error-or$result.effect-cne$case){throw 'Actual source changed its OLE effect'}}else{
     $expected=switch($case){'source-error'{'Original OLE failure'}'glide-error'{'Original glide failure'}'join-error'{'The file drag did not release'}}
     if(!$result.error-or!$result.error.Contains($expected)){throw 'Actual source swallowed its original OLE or glide failure'}
    }
    if([CapyFileDropTest.State]::Presses-ne1-or[CapyFileDropTest.State]::Forms-ne1-or[CapyFileDropTest.State]::Closes-ne1-or[CapyFileDropTest.State]::Disposals-ne1-or[CapyFileDropTest.State]::Releases-ne0){throw 'Actual source replayed input or omitted its owned Form cleanup'}
   }
  }
  $tokens=$null;$errors=$null
  $uia=[Management.Automation.Language.Parser]::ParseFile((Join-Path $PSScriptRoot '../scripts/CapyUia.ps1'),[ref]$tokens,[ref]$errors)
  foreach($name in 'Wait-Scale','Wait-Until','Read-Snapshot'){
   $function=$uia.Find({param($n)$n-is[Management.Automation.Language.FunctionDefinitionAst]-and$n.Name-eq$name},$true)
   . ([scriptblock]::Create($function.Extent.Text.Replace('[Diagnostics.Stopwatch]','[CapyFileDropTest.Watch]').Replace('[System.Windows.Automation.ElementNotAvailableException]','[CapyFileDropTest.UnavailableException]')))
  }
  $bound=$ast.Find({param($n)$n-is[Management.Automation.Language.AssignmentStatementAst]-and$n.Left.Extent.Text-eq'$CapyWaitSeconds'},$true)
  . ([scriptblock]::Create($bound.Extent.Text));$script:CapyWaitSeconds=$CapyWaitSeconds
  $review=$null;$script:CapyEach=$null;$run=$temp
  function Control($Target,[switch]$Arranged){if($Target-cne$script:dropCase.target-or!$Arranged-or[CapyFileDropTest.State]::Dpi-ne1){throw 'Drop changed its arranged native target lookup or DPI order'};[pscustomobject]@{Current=[pscustomobject]@{BoundingRectangle=[CapyFileDropTest.Rect]::new(100,200,80,60)}}}
  function Get-Process($Id){if($Id-ne$PID){throw 'Drop discovered a foreign host process'};[pscustomobject]@{Path='owned-powershell'}}
  function Start-Process($FilePath,[string[]]$ArgumentList,$WindowStyle,[switch]$PassThru){
   $script:dropCase.starts++
   if($FilePath-cne'owned-powershell'-or($ArgumentList[0..2]-join',')-cne'-NoProfile,-STA,-EncodedCommand'-or$WindowStyle-cne'Hidden'-or!$PassThru){throw 'Drop changed its owned STA source launch'}
   $command=[Text.Encoding]::Unicode.GetString([Convert]::FromBase64String($ArgumentList[3]));$t=$null;$e=$null
   $encodedSource=[Management.Automation.Language.Parser]::ParseInput($command,[ref]$t,[ref]$e)
   if($e.Count){throw 'Drop child command did not parse'}
   $arguments=$encodedSource.EndBlock.Statements[0].PipelineElements[0].CommandElements
   if((& ([scriptblock]::Create($arguments[-2].Extent.Text)))-cne$fileDragDefinition){throw 'Drop child lost its original native definition'}
   $payload=[Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($arguments[-1].Value))|ConvertFrom-Json
   if(($payload.paths-join'|')-cne'photo 日本語.png|saved drawing.capy'-or$payload.x-ne140-or$payload.y-ne230-or[IO.Path]::GetDirectoryName($payload.receipt)-cne$run){throw 'Drop child changed its physical tuple, file order or owned result directory'}
   if($script:dropCase.mode-notin'timeout','kill-error','stop-timeout','missing'){
    $result=@{effect=$(if($script:dropCase.mode-eq'None'){'None'}else{'Copy'});error=$(if($script:dropCase.mode-eq'error'){'Original source error'}else{$null})}
    [IO.File]::WriteAllText($payload.receipt,($result|ConvertTo-Json -Compress))
   }
   $script:dropCase.process=[CapyFileDropTest.Process]::new();$script:dropCase.process.FailKill=$script:dropCase.mode-eq'kill-error';$script:dropCase.process.FailWait=$script:dropCase.mode-eq'stop-timeout'
   $script:dropCase.process
  }
  function Write-Warning($Message){$script:dropCase.warnings.Add([string]$Message)}
  function Start-Sleep($Milliseconds){
   if($Milliseconds-ne50){throw 'Drop changed its existing polling interval'}
   if($script:dropCase.process.HandleReads-ne1){throw 'Drop observed its source before retaining the owned process handle'}
   [CapyFileDropTest.Watch]::Current.Elapsed=[CapyFileDropTest.Watch]::Current.Elapsed.Add([TimeSpan]::FromSeconds(1))
   if($script:dropCase.mode-notin'timeout','kill-error','stop-timeout'-and[CapyFileDropTest.Watch]::Current.Elapsed.TotalSeconds-ge2){$script:dropCase.process.HasExited=$true}
  }
  foreach($scale in 1,3){foreach($target in 'drawing-canvas','drawing-tabs'){foreach($mode in 'Copy','None','error','timeout','kill-error','stop-timeout','missing'){
   $env:CAPY_WAIT_SCALE=[string]$scale
   [CapyFileDropTest.State]::Reset();$script:dropCase=@{target=$target;mode=$mode;starts=0;process=$null;warnings=[Collections.Generic.List[string]]::new()}
   $failure=$null;$effect=$null;try{$effect=Drop-Files @('photo 日本語.png','saved drawing.capy') $target}catch{$failure=$_}
   $pending=$mode-in'timeout','kill-error','stop-timeout'
   if($mode-in'Copy','None'){if($failure-or$effect-cne$mode){throw "Actual drop changed its original returned effect: $mode, $failure"}}
   elseif($mode-eq'error'){if(!$failure-or$failure.Exception.Message-cne'Original source error'){throw 'Actual drop lost its original source failure'}}
   elseif($mode-eq'missing'){if(!$failure-or$failure.Exception.Message-cne'The file drag source exited without a result: 0'){throw 'Actual drop accepted a source without a result'}}
   else{
    $expected=if($target-eq'drawing-canvas'){'The canvas refused the dropped photo: None'}else{'The drawing tabs refused an image and a drawing: None'}
    if(!$failure-or$failure.Exception.Message-cne$expected-or[CapyFileDropTest.Watch]::Current.Elapsed.TotalSeconds-ne45*$scale){throw "Actual drop lost its original refusal or normal scaled deadline: $failure"}
   }
   if($script:dropCase.starts-ne1-or$script:dropCase.process.Kills-ne[int]$pending-or$script:dropCase.process.Disposals-ne1-or[CapyFileDropTest.State]::Releases-ne[int]$pending){throw 'Actual drop replayed a source, stopped another process, or changed pending-only contact cleanup'}
   $cleanupFailed=$mode-in'kill-error','stop-timeout'
   if($script:dropCase.process.Waits-ne[int]($pending-and$mode-ne'kill-error')-or(!$cleanupFailed-and$pending-and!$script:dropCase.process.HasExited)){throw 'Actual drop did not confirm its owned source termination within the cleanup bound'}
   if($script:dropCase.warnings.Count-ne[int]$cleanupFailed){throw 'Actual drop silently lost its secondary cleanup failure or warned on successful cleanup'}
   if($cleanupFailed){$expected=if($mode-eq'kill-error'){'Cleanup kill failure'}else{'The file drag source did not stop'};if(!$script:dropCase.warnings[0].Contains($expected)){throw 'Actual drop cleanup warning lost its original secondary failure'}}
  }}}
 }finally{$env:CAPY_WAIT_SCALE=$savedScale;[IO.Directory]::Delete($temp,$true)}
}
$priorContext=[CapyFileDragDpiProbe]::GetThreadDpiAwarenessContext()
$previous=[CapyFileDragDpiProbe]::SetThreadDpiAwarenessContext([IntPtr](-4))
if($previous -eq [IntPtr]::Zero){throw 'File drag DPI test could not acquire physical caller coordinates'}
$failure=$null;$receipt=$null;$shell=$null
try{
    $physical=[CapyFileDragDpiProbe]::Measure().PhysicalMetrics
    if($physical[2] -le 1 -or $physical[3] -le 1){throw 'File drag DPI test has no virtual display geometry'}
    $x=$physical[0]+[int]($physical[2]/2);$y=$physical[1]+[int]($physical[3]/2)
    $formsPreloaded=@([AppDomain]::CurrentDomain.GetAssemblies()|Where-Object {$_.GetName().Name -eq 'System.Windows.Forms'}).Count -ne 0
    $shell=[powershell]::Create();$shell.Runspace=[runspacefactory]::CreateRunspace();$shell.Runspace.ApartmentState='STA';$shell.Runspace.Open()
    $null=$shell.AddScript({param($X,$Y)
        $ErrorActionPreference='Stop'
        $stages=[ordered]@{before_forms_load=[CapyFileDragDpiProbe]::Measure()}
        Add-Type -AssemblyName System.Windows.Forms,System.Drawing
        $stages['after_forms_load']=[CapyFileDragDpiProbe]::Measure()
        $prior=[CapyFileDragDpiProbe]::GetThreadDpiAwarenessContext()
        $saved=[CapyFileDrag]::SetThreadDpiAwarenessContext([IntPtr](-4))
        if($saved -eq [IntPtr]::Zero){throw 'File drag STA test could not acquire physical coordinates'}
        $source=$null
        try{
            $stages['before_form_creation']=[CapyFileDragDpiProbe]::Measure()
            $source=[Windows.Forms.Form]@{FormBorderStyle='None';ShowInTaskbar=$false;TopMost=$true;StartPosition='Manual';Bounds=[Drawing.Rectangle]::new($X-40,$Y+160,80,80)}
            $stages['after_form_creation']=[CapyFileDragDpiProbe]::Measure()
            $source.Show();[Windows.Forms.Application]::DoEvents()
            $stages['after_form_show']=[CapyFileDragDpiProbe]::Measure()
            $sourceDpi=[CapyFileDragDpiProbe]::GetDpiForWindow($source.Handle);$systemDpi=[CapyFileDragDpiProbe]::GetDpiForSystem()
            if(!$sourceDpi -or !$systemDpi){throw 'File drag DPI test did not measure its source window and system DPI'}
            [CapyFileDragDpiProbe]::Reset()
            $sendMethod=[CapyFileDrag].GetMethod('Send',[Reflection.BindingFlags]'NonPublic,Static')
            $sendMethod.Invoke($null,[object[]]@([uint32]0,[int]$X,[int]$Y))|Out-Null
            $reference=[CapyFileDragDpiProbe]::Snapshot()[0]
            $glide=[CapyFileDrag]::Press($source.Handle,$X,$Y);[Windows.Forms.Application]::DoEvents()
            if(!$glide.Join(10000)){throw 'The real file drag glide thread did not release'}
            if([CapyFileDrag]::Failure){throw [CapyFileDrag]::Failure}
            [pscustomobject]@{stages=$stages;source_window_dpi=$sourceDpi;system_dpi=$systemDpi;reference=$reference;samples=@([CapyFileDragDpiProbe]::Snapshot())}
        } finally {
            if($source){$source.Close();$source.Dispose()}
            if([CapyFileDrag]::SetThreadDpiAwarenessContext($saved) -eq [IntPtr]::Zero -or ![CapyFileDragDpiProbe]::AreDpiAwarenessContextsEqual($prior,[CapyFileDragDpiProbe]::GetThreadDpiAwarenessContext())){throw 'File drag DPI test did not restore its STA context'}
        }
    }).AddArgument($x).AddArgument($y)
    $result=@($shell.Invoke());if($shell.HadErrors){throw ($shell.Streams.Error|Out-String)}
    if($result.Count -ne 1){throw 'File drag STA path did not return one measurement receipt'}
    $reference=$result[0].reference
    $worker=@($result[0].samples|Where-Object {$_.ThreadId -ne $reference.ThreadId})
    if(!$worker.Count -or @($worker.ThreadId|Select-Object -Unique).Count -ne 1){throw 'The real file drag glide path did not create one distinct native worker'}
    $release=$worker[-1]
    $metricsMatch=@($worker|Where-Object {($_.Metrics -join ',') -cne ($_.PhysicalMetrics -join ',')}).Count -eq 0
    $tupleMatches=$release.X -eq $reference.X -and $release.Y -eq $reference.Y
    $receipt=[ordered]@{powershell=$PSVersionTable.PSVersion.ToString();fixture_sha256=(Get-FileHash -LiteralPath $fixture -Algorithm SHA256).Hash;forms_preloaded=$formsPreloaded;source_window_dpi=$result[0].source_window_dpi;system_dpi=$result[0].system_dpi;source_window_scale=([double]$result[0].source_window_dpi/96);requested=@($x,$y);stages=$result[0].stages;reference=$reference;worker=$worker;worker_metrics_are_physical=$metricsMatch;worker_release_matches_caller_tuple=$tupleMatches;desktop_input=$false;real_source_form=$true}
    if(!$metricsMatch -or !$tupleMatches){throw 'File drag glide worker changed the physical coordinate space used by its caller'}
} catch {
    $failure=$_
} finally {
    if($shell){$shell.Runspace.Dispose();$shell.Dispose()}
    if([CapyFileDragDpiProbe]::SetThreadDpiAwarenessContext($previous) -eq [IntPtr]::Zero){throw 'File drag DPI test did not restore its caller context'}
    if(![CapyFileDragDpiProbe]::AreDpiAwarenessContextsEqual($priorContext,[CapyFileDragDpiProbe]::GetThreadDpiAwarenessContext())){throw 'File drag DPI test changed its caller context'}
    if($receipt){$receipt['caller_context_restored']=$true;$receipt['sta_context_restored']=$true;$receipt|ConvertTo-Json -Depth 8|Set-Content -LiteralPath $Output -Encoding UTF8}
}
if($failure){throw $failure}
Write-Output 'File drag real STA Form and glide-thread physical coordinates passed without desktop input'
