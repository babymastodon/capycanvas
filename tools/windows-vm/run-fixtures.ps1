param(
    [Parameter(Mandatory)][string]$Executable,
    [Parameter(Mandatory)][string]$Output,
    [string]$Name,
    [string]$DebuggerPath,
    [string]$BuildRecord,
    [string]$QueueConfig,
    [switch]$List,
    [ValidateSet('release','dev-perf')][string]$RustProfile='release',
    [ValidateRange(1,1440)][int]$TimeoutMinutes = 20,
    [switch]$HardwareGpu,
    [switch]$SharedSession
)
$ErrorActionPreference = 'Stop'
trap {
    New-Item -ItemType Directory -Force $Output | Out-Null
    ($_ | Out-String) + $_.ScriptStackTrace | Set-Content (Join-Path $Output 'runner-error.txt')
    exit 1
}
if(!$List -and !$env:CAPY_PRIVATE_DESKTOP){
    New-Item -ItemType Directory -Force $Output|Out-Null
    $runner=Join-Path $Output 'runner.json'
    [IO.File]::WriteAllText($runner+'.tmp',(@{process_id=$PID;process_start_utc=(Get-Process -Id $PID).StartTime.ToUniversalTime().ToString('o')}|ConvertTo-Json))
    [IO.File]::Move($runner+'.tmp',$runner)
}
. (Join-Path $PSScriptRoot 'session-identity.ps1')
. (Join-Path $PSScriptRoot '../../apps/layer-windows/scripts/FixtureJapaneseInput.ps1')
function Write-FixtureResult([string]$Path,[string]$Json){
    $stream=[IO.File]::Open($Path,[IO.FileMode]::OpenOrCreate,[IO.FileAccess]::Write,([IO.FileShare]::ReadWrite -bor [IO.FileShare]::Delete))
    $writer=[IO.StreamWriter]::new($stream,[Text.UTF8Encoding]::new($false))
    try{$null=$stream.Seek(0,[IO.SeekOrigin]::End);$writer.WriteLine($Json);$writer.Flush()}finally{$writer.Dispose();$stream.Dispose()}
}
function Write-FixturePlan([string]$Path,[string[]]$Runs){
    $plan=@{runs=@();timeout_minutes=$TimeoutMinutes;hardware_gpu=[bool]$HardwareGpu};if($Runs){$plan.runs=$Runs}
    $temporary=$Path+'.tmp'
    [IO.File]::WriteAllText($temporary,($plan|ConvertTo-Json),[Text.UTF8Encoding]::new($false))
    if(!('CapyFixturePlanFile' -as [type])){Add-Type -TypeDefinition @'
using System;using System.ComponentModel;using System.IO;using System.Runtime.InteropServices;using System.Text;using Microsoft.Win32.SafeHandles;
public static class CapyFixturePlanFile {
 [DllImport("kernel32.dll",CharSet=CharSet.Unicode,SetLastError=true)] static extern SafeFileHandle CreateFileW(string path,uint access,uint share,IntPtr security,uint creation,uint flags,IntPtr template);
 [DllImport("kernel32.dll",SetLastError=true)] static extern bool SetFileInformationByHandle(SafeFileHandle file,int kind,IntPtr info,uint size);
 public static void Publish(string source,string target){
  using(var file=CreateFileW(source,0x10000,7,IntPtr.Zero,3,0,IntPtr.Zero)){
   if(file.IsInvalid)throw new Win32Exception(Marshal.GetLastWin32Error());
   byte[] name=Encoding.Unicode.GetBytes(Path.GetFullPath(target));int offset=IntPtr.Size==8?20:12;
   IntPtr info=Marshal.AllocHGlobal(offset+name.Length+2);
   try{
    Marshal.WriteInt32(info,0,3);Marshal.WriteInt32(info,4,0);Marshal.WriteIntPtr(info,IntPtr.Size,IntPtr.Zero);Marshal.WriteInt32(info,offset-4,name.Length);Marshal.Copy(name,0,IntPtr.Add(info,offset),name.Length);
    Marshal.WriteInt16(info,offset+name.Length,0);
    if(!SetFileInformationByHandle(file,22,info,(uint)(offset+name.Length+2)))throw new Win32Exception(Marshal.GetLastWin32Error());
   }finally{Marshal.FreeHGlobal(info);}
  }
 }
}
'@}
    [CapyFixturePlanFile]::Publish($temporary,$Path)
}
function Get-FixtureGraphicsFailure($Exit,[string]$Message){
    if($Exit -eq 0){return}
    $text=([string]$Exit)+' '+$Message
    if($text -match '(?<![A-Za-z0-9_])(?:MILERR_NO_HARDWARE_DEVICE|0x0*8898008D|2291662989|-2003304307)(?![A-Za-z0-9_])'){return 'MILERR_NO_HARDWARE_DEVICE'}
}
function Write-FixtureSessionRecord([string]$Directory,[string]$Name,$Record){
    $path=Join-Path $Directory $Name;$temporary=$path+'.tmp'
    [IO.File]::WriteAllText($temporary,($Record|ConvertTo-Json -Depth 4),[Text.UTF8Encoding]::new($false))
    [CapyFixturePlanFile]::Publish($temporary,$path)
}
function Write-FixtureGraphicsWithdrawal([string]$Directory,$Result,$Identity){
    $request=@{};foreach($key in $Identity.Keys){$request[$key]=$Identity[$key]}
    $request.name=$Result.name;$request.exit=$Result.exit;$request.error=$Result.message
    $request.cause=$Result.infrastructure_cause;$request.time_utc=[DateTime]::UtcNow.ToString('o')
    Write-FixtureSessionRecord $Directory 'graphics-withdrawal.json' $request
}
function Wait-FixtureGraphicsEvidence([string]$Directory){
    $started=[DateTime]::UtcNow.ToString('o');$watch=[Diagnostics.Stopwatch]::StartNew()
    $path=Join-Path $Directory 'graphics-evidence-complete'
    while(!(Test-Path -LiteralPath $path) -and $watch.ElapsedMilliseconds -lt 360000){
        Start-Sleep -Milliseconds ([int][Math]::Max(0,[Math]::Min(1000,360000-$watch.ElapsedMilliseconds)))
    }
    Write-FixtureSessionRecord $Directory 'graphics-evidence-wait.json' @{started_utc=$started;completed_utc=[DateTime]::UtcNow.ToString('o');timeout=(!(Test-Path -LiteralPath $path));timeout_seconds=360}
}
function Get-FixtureTextScale{
    $path='HKCU:\Software\Microsoft\Accessibility'
    $key=if(Test-Path -LiteralPath $path){Get-Item -LiteralPath $path}
    $exists=$key -and 'TextScaleFactor' -in $key.GetValueNames()
    $kind=$null;$value=$null
    if($exists){$kind=$key.GetValueKind('TextScaleFactor').ToString();$value=$key.GetValue('TextScaleFactor',$null,[Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)}
    [pscustomobject]@{exists=[bool]$exists;kind=$kind;value=$value}
}
function Read-FixtureTextScale([int]$Milliseconds){
    $expression='[Windows.UI.ViewManagement.UISettings,Windows.UI.ViewManagement,ContentType=WindowsRuntime]::new().TextScaleFactor'
    $stdout=[IO.Path]::GetTempFileName();$stderr=[IO.Path]::GetTempFileName();$reader=$null
    try{
        $reader=Start-Process powershell.exe -ArgumentList @('-NoProfile','-NonInteractive','-Command',$expression) -NoNewWindow -PassThru -RedirectStandardOutput $stdout -RedirectStandardError $stderr
        $null=$reader.Handle
        if(!$reader.WaitForExit($Milliseconds)){& taskkill.exe /T /F /PID $reader.Id|Out-Null;throw 'UISettings text-scale reader exceeded its acknowledgement deadline'}
        if($reader.ExitCode -ne 0){throw ('UISettings text-scale reader failed: '+(Get-Content -LiteralPath $stderr -Raw))}
        [double](Get-Content -LiteralPath $stdout -Raw)
    }finally{
        if($reader){$reader.Dispose()}
        Remove-Item -LiteralPath $stdout,$stderr
    }
}
function Wait-FixtureTextScale($Saved,[string]$Phase){
    $expected=if($Saved.exists){[double]$Saved.value/100.0}else{1.0}
    $budget=15000*$(if($env:CAPY_WAIT_SCALE){[double]$env:CAPY_WAIT_SCALE}else{1})
    $watch=[Diagnostics.Stopwatch]::StartNew()
    do{
        try{$actual=Read-FixtureTextScale ([int][Math]::Max(1,$budget-$watch.Elapsed.TotalMilliseconds))}
        catch{throw "Fixture $Phase did not acknowledge saved Windows text scale ${expected}: $($_.Exception.Message)"}
        if($actual -eq $expected){return}
        Start-Sleep -Milliseconds 50
    }while($watch.Elapsed.TotalMilliseconds -lt $budget)
    throw "Fixture $Phase did not acknowledge saved Windows text scale $expected (UISettings.TextScaleFactor=$actual)"
}
function Restore-FixtureTextScale($Saved){
    if(((Get-FixtureTextScale)|ConvertTo-Json -Compress) -cne ($Saved|ConvertTo-Json -Compress)){
        $path='HKCU:\Software\Microsoft\Accessibility'
        if($Saved.exists){
            if(!(Test-Path -LiteralPath $path)){New-Item -Path $path|Out-Null}
            Set-ItemProperty -LiteralPath $path -Name TextScaleFactor -Type $Saved.kind -Value $Saved.value
        }else{Remove-ItemProperty -LiteralPath $path -Name TextScaleFactor}
        if(((Get-FixtureTextScale)|ConvertTo-Json -Compress) -cne ($Saved|ConvertTo-Json -Compress)){throw 'Windows text scale did not restore its original registry value'}
        [CapyVm.Native]::NotifyTextSize()
    }
    Wait-FixtureTextScale $Saved 'restoration'
}
function Get-FixtureLocalDumps{
    $path='SOFTWARE\Microsoft\Windows\Windows Error Reporting\LocalDumps'
    $parent=[Microsoft.Win32.Registry]::LocalMachine.OpenSubKey($path)
    $parentExists=$null -ne $parent;if($parent){$parent.Dispose()}
    $key=[Microsoft.Win32.Registry]::LocalMachine.OpenSubKey($path+'\CapyCanvas.exe')
    $saved=[ordered]@{parent_exists=$parentExists;exists=($null -ne $key);values=@{}}
    if($key){try{
        foreach($name in @('DumpFolder','DumpCount','DumpType')){
            if($name -in $key.GetValueNames()){$saved.values[$name]=@{kind=$key.GetValueKind($name);value=$key.GetValue($name,$null,[Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)}}
        }
    }finally{$key.Dispose()}}
    $saved
}
function Enable-FixtureLocalDumps($Saved){
    $folder=Join-Path $Output 'wer';New-Item -ItemType Directory -Force $folder|Out-Null
    $Saved|Export-Clixml -LiteralPath (Join-Path $folder 'registry-before.clixml')
    $dumps=Join-Path $folder 'dumps';New-Item -ItemType Directory -Force $dumps|Out-Null
    $key=[Microsoft.Win32.Registry]::LocalMachine.CreateSubKey('SOFTWARE\Microsoft\Windows\Windows Error Reporting\LocalDumps\CapyCanvas.exe')
    try{
        $key.SetValue('DumpFolder',$dumps,[Microsoft.Win32.RegistryValueKind]::ExpandString)
        $key.SetValue('DumpCount',3,[Microsoft.Win32.RegistryValueKind]::DWord)
        $key.SetValue('DumpType',2,[Microsoft.Win32.RegistryValueKind]::DWord)
    }finally{$key.Dispose()}
}
function Restore-FixtureLocalDumps($Saved){
    $path='SOFTWARE\Microsoft\Windows\Windows Error Reporting\LocalDumps'
    $key=[Microsoft.Win32.Registry]::LocalMachine.OpenSubKey($path+'\CapyCanvas.exe',$true)
    if($key){try{
        foreach($name in @('DumpFolder','DumpCount','DumpType')){
            if($Saved.values.ContainsKey($name)){$entry=$Saved.values[$name];$key.SetValue($name,$entry.value,$entry.kind)}else{$key.DeleteValue($name,$false)}
        }
        $empty=$key.ValueCount -eq 0 -and $key.SubKeyCount -eq 0
    }finally{$key.Dispose()}
        if(!$Saved.exists -and $empty){[Microsoft.Win32.Registry]::LocalMachine.DeleteSubKey($path+'\CapyCanvas.exe',$false)}
    }elseif($Saved.values.Count){throw 'The saved per-app LocalDumps key disappeared'}
    if(!$Saved.parent_exists){
        $key=[Microsoft.Win32.Registry]::LocalMachine.OpenSubKey($path)
        if($key){try{$empty=$key.ValueCount -eq 0 -and $key.SubKeyCount -eq 0}finally{$key.Dispose()}
            if($empty){[Microsoft.Win32.Registry]::LocalMachine.DeleteSubKey($path,$false)}
        }
    }
}
$Executable = if($List){[IO.Path]::GetFullPath($Executable)}else{(Resolve-Path -LiteralPath $Executable).Path}
$editorExecutable=$Executable
$buildManifest=if($BuildRecord){Get-Content -LiteralPath $BuildRecord -Raw|ConvertFrom-Json}else{$null}
if($buildManifest -and ($buildManifest.profile -ne $RustProfile -or [bool]$buildManifest.hardware_gpu -ne [bool]$HardwareGpu)){throw 'The fixture build record has a different profile or adapter mode'}
$controlBuilds=@{}
$scripts = (Resolve-Path (Join-Path $PSScriptRoot '../../apps/layer-windows/scripts')).Path
. (Join-Path $scripts 'CapyEnvironment.ps1')
. (Join-Path $PSScriptRoot 'test-hardware-traces.ps1')
function Get-FixtureVariantArguments([string]$Fixture,[string]$Option,[string]$Choice){
    if($Fixture -eq 'switcher-drag' -and $Option -eq 'ContextEnding'){'-ContextHoldOnly'}
    "-$Option";if($Choice){$Choice}
}
function ConvertTo-FixtureCommandLine([string[]]$Arguments){
    (@($Arguments|ForEach-Object {'"'+($_ -replace '(\\*)"','$1$1\"' -replace '(\\+)$','$1$1')+'"'}) -join ' ')
}
$runs = [ordered]@{ shortcuts = @{arguments=$null;hardware_gpu=$false} }
$debuggable = @()
foreach ($script in Get-ChildItem (Join-Path $scripts 'exercise-*.ps1')) {
    $fixture = $script.BaseName -replace '^exercise-'
    $parameters = [System.Management.Automation.Language.Parser]::ParseFile($script.FullName, [ref]$null, [ref]$null).ParamBlock.Parameters
    if ('Executable' -notin $parameters.Name.VariablePath.UserPath) { continue }
    if ('DebuggerPath' -in $parameters.Name.VariablePath.UserPath) { $debuggable += $fixture }
    $runs[$fixture] = @{arguments=@();hardware_gpu=$false}
    foreach ($parameter in $parameters) {
        $option = $parameter.Name.VariablePath.UserPath
        if ($parameter.StaticType -eq [switch]) { $runs["${fixture}:$option"] = @{arguments=@(Get-FixtureVariantArguments $fixture $option $null);hardware_gpu=$false} }
        $choices = ($parameter.Attributes | Where-Object { $_.TypeName.Name -eq 'ValidateSet' }).PositionalArguments.Value
        foreach ($choice in $choices | Where-Object { $null -ne $_ -and $_ -ne $parameter.DefaultValue.Value }) { $runs["${fixture}:$choice"] = @{arguments=@(Get-FixtureVariantArguments $fixture $option $choice);hardware_gpu=$false} }
    }
    if($fixture -eq 'popup'){
        $runs.Remove('popup');$runs.Remove('popup:light')
        foreach($group in 'controls','text-layout'){
            $runs["popup:${group}-light"]=@{arguments=@('-Group',$group,'-Theme','light');hardware_gpu=$false}
        }
    }
}
$runs.Remove('tool-bubbles:HdrOnly')
$runs.Remove('edge-reach:SweepOnly')
foreach($scenario in 'photo','photo-open','photo-collapsed','paint'){
    $runs.Remove("edge-reach:$scenario")
    foreach($theme in 'dark','light'){
        $runs["edge-reach:sweep-$scenario-$theme"]=@{arguments=@('-SweepOnly','-SweepScenario',$scenario,'-Theme',$theme);hardware_gpu=$false}
    }
}
foreach($hardwareSelector in @('drawers','drawers:light','toolbars:light','idle-brush','idle-brush:light','touch-gestures','touch-gestures:Simple','touch-gestures:SimpleLight','touch-gestures:ThreeFinger','touch-gestures:layers','localization:ColorOnly','tool-variations:sketch','tool-variations:sketch-light','color-picker:light','tool-bubbles:EraserOnly','tool-bubbles:EraserLightOnly','tool-bubbles:blenders-dark','tool-bubbles:blenders-light','tool-bubbles:BlendersOnly','tool-bubbles:sliders-dark','tool-bubbles:sliders-light','tool-bubbles:SlidersOnly','lifecycle:RecoverGpu','multiwindow:RecoverGpu')){
    if(!$runs.Contains($hardwareSelector)){throw "Unknown hardware fixture: $hardwareSelector"}
    $runs[$hardwareSelector].hardware_gpu=$true
}
foreach($unrendered in @('tool-bubbles:ObserverOnly')){
    if(!$runs.Contains($unrendered)){throw "Unknown unrendered fixture: $unrendered"}
    $runs[$unrendered].renders=$false
}
function Get-FixtureProcessArguments([string]$Fixture,[string[]]$Arguments){
    if($Fixture -cne 'snap'){return $Arguments}
    $parameters=@()
    for($i=2;$i -lt $Arguments.Count;$i++){
        $name=$Arguments[$i].TrimStart('-').Replace("'","''");$value='$true'
        if($i+1 -lt $Arguments.Count -and !$Arguments[$i+1].StartsWith('-')){$i++;$value="'"+$Arguments[$i].Replace("'","''")+"'"}
        $parameters+="'$name'=$value"
    }
    '-Command';"& '"+(Join-Path $scripts 'test-desktop.ps1').Replace("'","''")+"' -Desktop Default -Script '"+$Arguments[1].Replace("'","''")+"' -Parameters @{"+($parameters -join ';')+"} -TimeoutSeconds "+($TimeoutMinutes*60)
}
function Select-Fixtures([string[]]$Available,[string[]]$Requested) {
    if($null -eq $Requested -or !$Requested.Count){return $Available}
    $Requested=@($Requested|ForEach-Object {
        switch($_){
            'tool-bubbles:HdrOnly'{'tool-bubbles:hdr-dark';'tool-bubbles:hdr-light'}
            'header-settings'{'header:catalog-only';'header:catalog-only-light'}
            'edge-reach:SweepOnly'{foreach($scenario in 'photo','photo-open','photo-collapsed','paint'){foreach($theme in 'dark','light'){"edge-reach:sweep-$scenario-$theme"}}}
            'smoke'{
                'activation:default','canvas-editing:smoke','canvas-editing:smoke-light','clipboard:default',
                'color-editor:light','column-stacks:touch','documents:new-drawing','documents:native-writeback-light',
                'drawers:default','drawers:light','editor:default','effects:light',
                'header:default','header:light','image-rows:default','layer-pickup:default',
                'layers:default','layers:PaperLight','localization-smoke:light','navigator:default',
                'navigator:light','persistence:default','popup:default','popup:light',
                'saved-data:light','scopes:default','selection:light','startup-close:default',
                'switcher-drag:mouse','switcher:default','tab-drag:default','tab-drag:light',
                'tool-bubbles:light-pen','toolbar-components:default','window:default','window:light',
                'workspace-layout:default','workspace-layout:light','workspace-pickup:default','zoom-readout:touch'
            }
            default{$_}
        }
    })
    $Requested=@($Requested|ForEach-Object {
        switch($_){
            'popup:default'{foreach($group in 'controls','text-layout'){"popup:$group"}}
            'popup:light'{foreach($group in 'controls','text-layout'){"popup:${group}-light"}}
            default{$_}
        }
    })
    $defaults = @($Requested | Where-Object { $_ -match '^[^:]+:default$' } | ForEach-Object { ($_ -split ':')[0] })
    foreach ($item in $Requested) {
        $known = if ($item -match '^([^:]+):default$') { $Matches[1] -in $Available } else {
            @($Available | Where-Object { $_ -eq $item -or ($_ -split ':')[0] -eq $item }).Count -gt 0
        }
        if (!$known) { throw "Unknown fixture: $item" }
    }
    @($Available | Where-Object { !$Requested -or $_ -in $Requested -or ($_ -split ':')[0] -in $Requested -or $_ -in $defaults })
}
function Get-NextFixture([string[]]$Planned,$Queue,[Collections.Generic.List[string]]$Claimed){
    if($Queue){
        try{$next=(Invoke-RestMethod -Method Post -Uri $Queue.url -Headers @{Authorization=('Bearer '+$Queue.token)} -TimeoutSec 30).selector}
        catch{throw 'The fixture queue request failed.'}
    }else{$next=if($Claimed.Count -lt $Planned.Count){$Planned[$Claimed.Count]}else{$null}}
    if($null -eq $next){return}
    if($next -isnot [string] -or $next -notin $Planned -or $Claimed.Contains($next)){throw 'The fixture queue returned an unknown or repeated selector.'}
    $Claimed.Add($next)
    $next
}
function Control-Fixture([string]$Fixture,[string[]]$Arguments=@()){
    if($Fixture -eq 'layer-pickup' -and '-PendingDropOnly' -in $Arguments){return 'LayerDrop'}
    switch($Fixture){'button-feedback'{'Feedback'} 'sdk-feedback'{'Feedback'} 'scroll-indicators'{'Feedback'} 'navigator-allocation'{'Feedback'} 'popup'{'Popup'} default{$null}}
}
function Get-FixtureCompanionArguments([string]$Fixture,[string[]]$Arguments,[scriptblock]$BuildControl){
    if($Fixture -eq 'layer-pickup' -and '-PendingDropOnly' -notin $Arguments -and '-ClippedEdgesOnly' -notin $Arguments -and '-HandoffOnly' -notin $Arguments){'-DropFixture';& $BuildControl 'LayerDrop'}
}
$selected = if ($Name) { $Name -split ',' } else { @() }
$planned = @(Select-Fixtures $runs.Keys $selected)
if (!$planned.Count) { throw 'No fixtures selected.' }
if($List){
    $requirements=[ordered]@{}
    foreach($name in $planned){
        $fixture=($name -split ':')[0];$arguments=$runs[$name].arguments
        $controls=@(@(Control-Fixture $fixture $arguments;Get-FixtureCompanionArguments $fixture $arguments {param($kind)$kind}|Where-Object {$_ -notlike '-*'})|Where-Object {$_}|Sort-Object -Unique)
        $requirements[$name]=@{hardware_gpu=$runs[$name].hardware_gpu;controls=$controls}
    }
    $controls=@($requirements.Values|ForEach-Object {$_.controls}|Sort-Object -Unique)
    @{runs=$planned;controls=$controls;requirements=$requirements}|ConvertTo-Json -Depth 5 -Compress;exit
}
function Get-FixtureTimeoutSeconds([int]$Count,[int]$Minutes){
    [int][Math]::Min(86400,[long]$Count*($Minutes*60+120))
}
if(!$env:CAPY_PRIVATE_DESKTOP){
    & (Join-Path $PSScriptRoot '../../apps/layer-windows/scripts/test-desktop.ps1') -Script $PSCommandPath -Parameters $PSBoundParameters -TimeoutSeconds (Get-FixtureTimeoutSeconds $planned.Count $TimeoutMinutes)
    exit
}
Add-Type -AssemblyName System.Drawing, System.Windows.Forms
Add-Type -Namespace CapyVm -Name Native -MemberDefinition @'
[DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr value);
[DllImport("user32.dll")] public static extern uint GetDpiForSystem();
[DllImport("kernel32.dll")] public static extern IntPtr GetConsoleWindow();
[DllImport("kernel32.dll")] public static extern bool SetConsoleCtrlHandler(IntPtr handler, bool add);
[DllImport("user32.dll")] public static extern IntPtr GetSystemMenu(IntPtr window, bool revert);
[DllImport("user32.dll")] public static extern bool DeleteMenu(IntPtr menu, uint position, uint flags);
[DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr window, IntPtr after, int x, int y, int width, int height, uint flags);
[DllImport("user32.dll",CharSet=CharSet.Unicode)] static extern IntPtr SendMessageTimeout(IntPtr window,uint message,UIntPtr w,string l,uint flags,uint timeout,out UIntPtr result);
public static void NotifyTextSize(){UIntPtr result;SendMessageTimeout(new IntPtr(0xffff),0x1a,UIntPtr.Zero,"Accessibility",2,2000,out result);}
'@
[CapyVm.Native]::SetProcessDpiAwarenessContext([IntPtr]-4) | Out-Null
[CapyVm.Native]::SetConsoleCtrlHandler([IntPtr]::Zero, $true) | Out-Null
$console = [CapyVm.Native]::GetConsoleWindow()
[CapyVm.Native]::DeleteMenu([CapyVm.Native]::GetSystemMenu($console, $false), 0xF060, 0) | Out-Null
$area = [System.Windows.Forms.Screen]::PrimaryScreen.WorkingArea
[CapyVm.Native]::SetWindowPos($console, [IntPtr]::Zero, $area.Right - 640, $area.Bottom - 240, 640, 240, 0x14) | Out-Null

function Save-Screen([string]$Path) {
    $bounds = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
    $bitmap = [System.Drawing.Bitmap]::new($bounds.Width, $bounds.Height)
    $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
    try {
        $graphics.CopyFromScreen($bounds.Location, [System.Drawing.Point]::Empty, $bounds.Size)
        $bitmap.Save($Path)
    } finally {
        $graphics.Dispose()
        $bitmap.Dispose()
    }
}

function Reviews([string]$ContextPath,[Collections.Generic.List[object]]$Failures) {
    $sessionId=[Diagnostics.Process]::GetCurrentProcess().SessionId
    $sameIdentity={
        param($row,$owner,$start)
        if(!$row){return $false}
        if(($row.SessionId -isnot [uint32] -and $row.SessionId -isnot [int]) -or $row.SessionId -lt 0){throw "Fixture review $($owner.process_id) snapshot session is unavailable"}
        if($row.SessionId -ne $sessionId){return $false}
        if(!$owner.process_start_utc){throw "Fixture review $($owner.process_id) has no recorded start time"}
        if($row.CreationDate -isnot [DateTime] -or $row.CreationDate.Ticks%10L){throw "Fixture review $($owner.process_id) snapshot creation time is unavailable"}
        $row.CreationDate.ToUniversalTime().Ticks -eq ($start.Ticks-($start.Ticks%10L))
    }
    $files=if($ContextPath){Get-Item -LiteralPath $ContextPath}else{Get-ChildItem -LiteralPath $Output -Filter '*-context.json' -File}
    $contexts=@(foreach($file in $files){
        try{Get-Content -LiteralPath $file.FullName -Encoding UTF8 -Raw|ConvertFrom-Json}
        catch{if($null -eq $Failures){throw};$Failures.Add($_)}
    })
    $closed=@($contexts|ForEach-Object {$_.closes}|Where-Object {$_.result -ceq 'pass' -and $_.window_only -is [bool] -and !$_.window_only -and $_.process_start_utc})
    $owners=@($contexts|ForEach-Object {@($_)+@($_.windows)})|Where-Object {$_.process_id}|Group-Object process_id,process_start_utc|ForEach-Object {$_.Group[0]}
    if($ContextPath -and !$owners){throw 'Timed-out fixture context has no recorded review identity'}
    $snapshot=@{}
    foreach($row in @(if($owners){Get-CimInstance Win32_Process -Property ProcessId,SessionId,CreationDate -ErrorAction Stop})){
        if(($row.ProcessId -isnot [uint32] -and $row.ProcessId -isnot [int]) -or $row.ProcessId -lt 0 -or $snapshot.ContainsKey([int]$row.ProcessId)){throw 'Fixture review snapshot process identity is invalid'}
        $snapshot[[int]$row.ProcessId]=$row
    }
    $matched=[Collections.Generic.List[object]]::new()
    try{foreach($owner in $owners){
      try{
        $id=[int]$owner.process_id;if($id -le 0){throw 'Fixture review process identity is invalid'}
        if($owner.process_start_utc){
            $start=([DateTime]$owner.process_start_utc).ToUniversalTime()
            if(@($closed|Where-Object {$_.process_id -eq $owner.process_id -and ([DateTime]$_.process_start_utc).ToUniversalTime() -eq $start}).Count){continue}
        }
        if(!(& $sameIdentity $snapshot[$id] $owner $start)){continue}
        $process=$null;$retained=$false
        try{
            try{
                $process=Get-Process -Id $id -ErrorAction Stop
                $handle=$process.Handle
                if($null -eq $handle -or [IntPtr]$handle -eq [IntPtr]::Zero){throw "Fixture review $id process handle is unavailable"}
                $actualStart=$process.StartTime
                if($actualStart -isnot [DateTime]){throw "Fixture review $id creation time is unavailable"}
                if($actualStart.ToUniversalTime().Ticks -ne $start.Ticks){throw "Fixture review $id creation time does not match"}
                $retained=$true;$matched.Add($process)
            }catch{
                $failure=$_
                $exited=$false
                if($null -ne $process){try{$exited=$process.HasExited}catch{}}
                if($exited -is [bool] -and $exited){continue}
                $fresh=Get-CimInstance Win32_Process -Filter "ProcessId=$id" -Property ProcessId,SessionId,CreationDate -ErrorAction Stop
                if(& $sameIdentity $fresh $owner $start){throw $failure}
            }
        }finally{if($null -ne $process -and !$retained){$process.Dispose()}}
      }catch{if($null -eq $Failures){throw};$Failures.Add($_)}
    }}catch{foreach($process in $matched){$process.Dispose()};throw}
    $matched.ToArray()
}
function Stop-Review {
    $watch=[Diagnostics.Stopwatch]::StartNew()
    $failures=[Collections.Generic.List[object]]::new()
    $owned = @(Reviews -Failures $failures)
    try{
        foreach($process in $owned){
            try{Stop-Process -InputObject $process -Force}catch{$failures.Add($_)}
        }
        foreach($process in $owned){
            try{if(!$process.WaitForExit([int][Math]::Max(0,30000-$watch.Elapsed.TotalMilliseconds))){throw "Fixture review $($process.Id) did not exit within 30 seconds"}}catch{$failures.Add($_)}
        }
    }finally{foreach($process in $owned){try{$process.Dispose()}catch{$failures.Add($_)}}}
    if($failures.Count){throw (($failures|ForEach-Object {$_.Exception.Message}) -join '; ')}
}
function Wait-FixtureReviews([string]$Log) {
    $watch=[Diagnostics.Stopwatch]::StartNew()
    $owned=@(Reviews "$Log-context.json")
    try{
        foreach($process in $owned){
            $remaining=[int][Math]::Max(0,30000-$watch.Elapsed.TotalMilliseconds)
            if(!$process.WaitForExit($remaining)){throw "Fixture review $($process.Id) did not exit within 30 seconds"}
        }
    }finally{foreach($process in $owned){$process.Dispose()}}
}
function Save-Evidence([string]$Log,[bool]$Renders=$true,[switch]$PreserveReviews) {
    $rendered = $Renders -and $HardwareGpu -and !$controlFixture
    $evidence = "$Log-evidence"
    $desktopRuns=@(if(Test-Path -LiteralPath "$Log.log"){Get-Content -LiteralPath "$Log.log"|ForEach-Object {if($_ -cmatch '^Private desktop artifacts: (.+)$'){$Matches[1]}}})|Select-Object -Unique
    foreach($recorded in $desktopRuns){
        $desktopRun=[IO.Path]::GetFullPath($recorded)
        $desktopRoot=[IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../../artifacts/windows/test-desktop'))
        $id=[IO.Path]::GetFileName($desktopRun)
        if([IO.Path]::GetDirectoryName($desktopRun) -ine $desktopRoot -or $id -notmatch '^[a-f0-9]{32}$'){throw "Invalid private desktop evidence path: $recorded"}
        & robocopy.exe $desktopRun (Join-Path $evidence ('test-desktop/'+$id)) /E /R:0 /W:0 /NFL /NDL /NJH /NJS|Out-Null
        if($LASTEXITCODE -ge 8){throw "Could not preserve private desktop evidence: $desktopRun"}
    }
    if (!(Test-Path "$Log-context.json")) {
        if ($rendered) { throw "Missing fixture context: $Log-context.json" }
        return
    }
    $context = Get-Content "$Log-context.json" -Encoding UTF8 -Raw | ConvertFrom-Json
    New-Item -ItemType Directory -Force $evidence | Out-Null
    $owners = @($context.process_id) + @($context.windows | ForEach-Object { $_.process_id }) + @(Reviews | ForEach-Object {try{$_.Id}finally{$_.Dispose()}})
    $folders = @($context.trace_directory, $context.run, (Split-Path $Executable)) + @($context.windows | ForEach-Object { $_.trace_directory }) | Where-Object { $_ } | Select-Object -Unique
    foreach ($folder in $folders) {
        foreach ($owner in $owners | Where-Object { $_ } | Select-Object -Unique) {
            Get-ChildItem -LiteralPath $folder -File | Where-Object {
                $_.Extension -ne '.pending' -and $_.Name -match "^(ui-state|camera-state|presentation-state|search-state|windows|prediction|latency)-$owner(-|\.)"
            } | Copy-Item -Destination $evidence
        }
    }
    if(!$PreserveReviews){Stop-Review}
    if ($context.run -and (Test-Path -LiteralPath $context.run)) {
        & robocopy.exe $context.run $evidence /E /R:0 /W:0 /NFL /NDL /NJH /NJS | Out-Null
        if ($LASTEXITCODE -ge 8) { throw "Could not preserve fixture evidence: $($context.run)" }
    }
    if($rendered){
        $scales=@(Get-ChildItem -LiteralPath $evidence -Filter 'ui-state-*.json'|ForEach-Object {
            $snapshot=Get-Content -LiteralPath $_.FullName -Raw|ConvertFrom-Json
            if($null -ne $snapshot.model.layout_input.cache.context){@{file=$_.Name;process_id=$snapshot.process_id;window_id=$snapshot.window_id;scale=$snapshot.model.layout_input.cache.context.scale}}
        })
        @{context_scales=$scales;expected_scale=$buildManifest.fixture_scale}|ConvertTo-Json -Depth 5|Set-Content -LiteralPath "$Log-display.json"
        if($buildManifest.fixture_scale -and (!$scales.Count -or @($scales|Where-Object {$_.scale -ne $buildManifest.fixture_scale}).Count)){throw 'Hardware fixture runtime scale differs from the round display'}
        Assert-HardwareTraces -Directory $evidence -Output "$Log-adapters.json"
    }
}

function Start-Shortcuts {
    . (Join-Path $scripts 'CapyUia.ps1')
    $saved = @{}
    foreach ($variable in 'CAPY_SMOKE_TEST', 'CAPY_TRACE_UI', 'CAPY_STORAGE_DIR') { $saved[$variable] = [Environment]::GetEnvironmentVariable($variable) }
    $env:CAPY_SMOKE_TEST = '1'
    $env:CAPY_TRACE_UI = '1'
    $env:CAPY_STORAGE_DIR = Join-Path $Output 'shortcuts-profile'
    try { $review = Start-Process $Executable -WorkingDirectory (Split-Path $Executable) -PassThru } finally { foreach ($entry in $saved.GetEnumerator()) { Restore-CapyEnvironmentValue $entry.Key $entry.Value } }
    $directory = Split-Path $Executable
    @{process_id=$review.Id;process_start_utc=$review.StartTime.ToUniversalTime().ToString('o');trace_directory=$directory;profile=(Join-Path $Output 'shortcuts-profile')} | ConvertTo-Json | Set-Content -LiteralPath $env:CAPY_FIXTURE_CONTEXT
    $deadline = (Get-Date).AddMinutes(2)
    while ($true) {
        $review.Refresh()
        $drawing = Owned-DrawingWindow $review -TraceDirectory $directory
        $state = if ($drawing) { Get-Item -LiteralPath (Join-Path $directory "ui-state-$($review.Id)-$($drawing.Handle.ToInt64()).json") -ErrorAction SilentlyContinue }
        if ($state -and $state.LastWriteTimeUtc -ge $review.StartTime.ToUniversalTime()) { break }
        if ((Get-Date) -gt $deadline) { throw 'The shortcuts review did not publish its UI state.' }
        Start-Sleep -Milliseconds 250
    }
    @('-File', (Join-Path $scripts 'exercise-shortcuts.ps1'), '-ProcessId', $review.Id, '-StateFile', $state.FullName)
}

function Assert-FixtureFiles([string]$Directory,$Files){
    if(!$Files -or !$Files.'CapyCanvas.exe' -or !$Files.'layer_windows.dll'){throw 'The control fixture has no complete build record'}
    foreach($entry in $Files.PSObject.Properties){
        $path=Join-Path $Directory $entry.Name
        if(!(Test-Path -LiteralPath $path) -or (Get-FileHash -LiteralPath $path).Hash -cne $entry.Value){throw "Control fixture binary differs from its build record: $($entry.Name)"}
    }
}
function Build-ControlFixture([string]$controlFixture){
    if(!$controlBuilds.ContainsKey($controlFixture)){
        $directory=(Split-Path $editorExecutable)+'-'+$controlFixture
        if($buildManifest){Assert-FixtureFiles $directory $buildManifest.controls.$controlFixture}
        else{
            $intermediate=(Split-Path $editorExecutable) -replace '-dev-perf$',''
            & (Join-Path $scripts 'build.ps1') -Configuration Release -RustProfile $RustProfile -ControlFixture $controlFixture -SoftwareAdapterTests:(!$HardwareGpu) -SkipRust -SkipRestore -OutputDirectory $directory -IntermediateDirectory ($intermediate+'.obj') *> (Join-Path $Output ('build-'+$controlFixture+'.log'))
        }
        $controlBuilds[$controlFixture]=Join-Path $directory 'CapyCanvas.exe'
        @($controlBuilds.GetEnumerator()|ForEach-Object {
            $binaryDirectory=Split-Path $_.Value
            @{fixture=$_.Key;profile=$RustProfile;hardware_gpu=[bool]$HardwareGpu;source_provenance='provenance.json';files=@('CapyCanvas.exe','layer_windows.dll'|ForEach-Object {@{name=$_;sha256=(Get-FileHash -LiteralPath (Join-Path $binaryDirectory $_)).Hash}})}
        })|ConvertTo-Json -Depth 5|Set-Content (Join-Path $Output 'control-builds.json')
    }
    $controlBuilds[$controlFixture]
}

New-Item -ItemType Directory -Force $Output | Out-Null
"$($area.Width)x$($area.Height) work area at $([CapyVm.Native]::GetDpiForSystem()) dpi" | Set-Content (Join-Path $Output 'display.txt')
if ($HardwareGpu) {
    $env:LAYER_TEST_SOFTWARE_GPU = $null
} else {
    $env:LAYER_TEST_SOFTWARE_GPU = '1'
}
$env:NO_COLOR = '1'
Write-FixturePlan (Join-Path $Output 'plan.json') @(if(!$QueueConfig){$planned})
$queue=if($QueueConfig){Get-Content -LiteralPath $QueueConfig -Raw|ConvertFrom-Json}else{$null}
if($QueueConfig -and (!$queue.url -or !$queue.token)){throw 'The fixture queue configuration is incomplete.'}
$env:CAPY_WAIT_SCALE = if($HardwareGpu -and !$SharedSession){$null}else{'3'}
$textScale=[pscustomobject]@{exists=$false;kind=$null;value=$null}
Restore-FixtureTextScale $textScale
$fixtureSessionIdentity=if($SharedSession){Get-CapySessionIdentity}else{$null}
if($fixtureSessionIdentity){Write-FixtureSessionRecord $Output 'graphics-identity.json' $fixtureSessionIdentity}
$claimed=[Collections.Generic.List[string]]::new()
$results = Join-Path $Output 'results.jsonl'
$graphicsCause=$null
while($null -ne ($name=Get-NextFixture $planned $queue $claimed)){
    $run=[pscustomobject]@{Key=$name;Value=$runs[$name].arguments}
    if($queue){Write-FixturePlan (Join-Path $Output 'plan.json') $claimed.ToArray()}
    $fixture = ($run.Key -split ':')[0]
    Stop-Review
    Remove-FixtureJapaneseInput
    $log = Join-Path $Output ($run.Key -replace ':', '-')
    $env:CAPY_FIXTURE_CONTEXT = "$log-context.json"
    $started = Get-Date
    $cleanupError=$null;$retirementError=$null;$timedOut=$false;$graphicsCause=$null;$graphicsExit=$null;$errors=@()
    try {
      try {
        Wait-FixtureTextScale $textScale 'setup'
        $controlFixture=Control-Fixture $fixture $run.Value
        $Executable=$editorExecutable
        if($controlFixture){
            $Executable=Build-ControlFixture $controlFixture
        }
        if ($DebuggerPath -and $fixture -notin $debuggable) { throw "Fixture $fixture does not support console debugging." }
        $arguments = if ($null -eq $run.Value) { Start-Shortcuts } else { @('-File', (Join-Path $scripts "exercise-$fixture.ps1"), '-Executable', $Executable) + $run.Value }
        $arguments += @(Get-FixtureCompanionArguments $fixture $run.Value ${function:Build-ControlFixture})
        if ($DebuggerPath) { $arguments += @('-DebuggerPath', $DebuggerPath) }
        $arguments=@(Get-FixtureProcessArguments $fixture $arguments)
        $process = Start-Process (Get-Process -Id $PID).Path -ArgumentList (ConvertTo-FixtureCommandLine (@('-NoProfile', '-Sta') + $arguments)) -NoNewWindow -PassThru -RedirectStandardOutput "$log.log" -RedirectStandardError "$log.err"
        $null = $process.Handle
        $process.PriorityClass='High'
        $finished = $process.WaitForExit($TimeoutMinutes * 60000)
        if (!$finished) {
            $timedOut=$true
            try{Save-Screen "$log.png"}finally{& taskkill.exe /T /F /PID $process.Id | Out-Null}
        }
        $exit = if ($finished) { $process.ExitCode } else { 'timeout' }
        $errors = @(Get-Content "$log.err")
        $marker = [array]::FindLastIndex([string[]]$errors, [Predicate[string]]{ param($line) $line -match '^\s*\|\s*~' })
        $message = (@($errors | Select-Object -Skip ($marker + 1)) -match '\S' -replace '^\s*\|\s*', '') -join ' '
      } catch {
        $exit = if($timedOut){'timeout'}else{'runner'}
        $message = $_.Exception.Message
      }
      $graphicsCause=Get-FixtureGraphicsFailure $exit ($message+' '+($errors -join ' '))
      if($graphicsCause){$graphicsExit=$exit}
      try {
        if ($exit -ne 0 -and !(Test-Path "$log.png")) { Save-Screen "$log.png" }
        Save-Evidence $log ($runs[$run.Key].renders -ne $false) -PreserveReviews:([bool]($SharedSession -and $graphicsCause))
      } catch {
        if(!$timedOut){$exit = 'evidence'}
        if(!$graphicsCause){$graphicsCause=Get-FixtureGraphicsFailure $exit $_.Exception.Message;$graphicsExit=$exit}
        $message += " $($_.Exception.Message)"
      }
    } finally {
        if($timedOut){try{Wait-FixtureReviews $log}catch{$retirementError=$_.Exception.Message}}
        if($textScale){try{Restore-FixtureTextScale $textScale}catch{$cleanupError=$_.Exception.Message}}
    }
    if($cleanupError){
        $message=(@($message,"Text-scale cleanup failed (fixture exit ${exit}): $cleanupError")|Where-Object {$_}) -join ' '
        if(!$timedOut){$exit='cleanup'}
    }
    if($retirementError){$message=(@($message,"Session retired after timeout: $retirementError")|Where-Object {$_}) -join ' '}
    if($graphicsCause){$exit=$graphicsExit}
    $result = [ordered]@{ name = $run.Key; exit = $exit; seconds = [math]::Round(((Get-Date) - $started).TotalSeconds); message = $message }
    if($graphicsCause){$result.infrastructure_invalid=$true;$result.infrastructure_cause=$graphicsCause}
    Write-FixtureResult $results ($result|ConvertTo-Json -Compress)
    if($graphicsCause -and $SharedSession){Write-FixtureGraphicsWithdrawal $Output $result $fixtureSessionIdentity;Wait-FixtureGraphicsEvidence $Output;break}
    if($retirementError){throw "Session retired after timeout: $retirementError"}
    if($cleanupError){throw "Fixture text-scale cleanup failed: $cleanupError"}
}
Stop-Review
Set-Content (Join-Path $Output 'complete') ''
