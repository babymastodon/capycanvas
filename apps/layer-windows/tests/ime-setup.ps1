param([string]$Fixture=(Join-Path $PSScriptRoot '../scripts/exercise-ime.ps1'))
$ErrorActionPreference='Stop'
Add-Type -AssemblyName UIAutomationClient,UIAutomationTypes
Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
public static class ImeSetupProbe {
 public static string Focused,Value,Expected,Romaji,Typed;
 public static bool Rename,Selected,PartialSelection,Mode;
 public static int Language,ModeCalls;
 public static readonly List<string> Calls=new List<string>();
 public static void Reset() {Focused="drawing-canvas";Value="Current ink";Expected="";Romaji="";Typed="";Rename=false;Selected=false;PartialSelection=false;Mode=false;Language=0;ModeCalls=0;Calls.Clear();}
 public static void Owner(IntPtr window,uint process) {if(window!=new IntPtr(71)||process!=7)throw new Exception("IME setup changed its owned window");}
}
public static class CapyIme {
 public static int Language(IntPtr window,uint process) {ImeSetupProbe.Owner(window,process);return ImeSetupProbe.Language;}
 public static void Open(IntPtr window,uint process) {ImeSetupProbe.Owner(window,process);ImeSetupProbe.Calls.Add("open");}
}
public static class CapyRowPointer {
 public static void Chord(uint process,ushort[] modifiers,ushort key) {
  if(process!=7||modifiers.Length!=1)throw new Exception("IME chord ownership changed");
  if(modifiers[0]==91&&key==32) {ImeSetupProbe.Language=0x411;ImeSetupProbe.Calls.Add("profile");return;}
  if(modifiers[0]!=17)throw new Exception("Unexpected IME chord modifier");
  if(key==20) {
   if(ImeSetupProbe.Rename||ImeSetupProbe.Focused=="layer-opacity")throw new Exception("IME mode setup reached an active text editor");
   ImeSetupProbe.Mode=true;ImeSetupProbe.ModeCalls++;ImeSetupProbe.Calls.Add("mode");return;
  }
  if(key!=65||!ImeSetupProbe.Rename&&ImeSetupProbe.Focused!="layer-opacity")throw new Exception("Unexpected IME selection chord");
  ImeSetupProbe.Selected=!ImeSetupProbe.PartialSelection;ImeSetupProbe.Typed="";ImeSetupProbe.Calls.Add("select");
 }
 public static void Key(uint process,ushort key) {
  if(process!=7)throw new Exception("IME key ownership changed");
  if(key==113) {ImeSetupProbe.Rename=true;ImeSetupProbe.Calls.Add("rename");return;}
  if(key==32) {ImeSetupProbe.Value="\u65e5\u672c";ImeSetupProbe.Calls.Add("candidate");return;}
  if(key<65||key>90||!ImeSetupProbe.Mode||!ImeSetupProbe.Selected)throw new Exception("Romaji reached an editor without full native selection and prepared IME mode");
  ImeSetupProbe.Typed+=(char)key;
  if(!ImeSetupProbe.Romaji.StartsWith(ImeSetupProbe.Typed,StringComparison.Ordinal))throw new Exception("Genuine romaji key sequence changed");
  if(ImeSetupProbe.Typed==ImeSetupProbe.Romaji)ImeSetupProbe.Value=ImeSetupProbe.Expected;
 }
}
'@
$tokens=$null;$errors=$null
$ast=[Management.Automation.Language.Parser]::ParseFile($Fixture,[ref]$tokens,[ref]$errors)
if(@($errors).Count){throw ($errors|Out-String)}
foreach($definition in $ast.FindAll({param($node)$node -is [Management.Automation.Language.FunctionDefinitionAst]},$true)){. ([scriptblock]::Create($definition.Extent.Text))}
$controls=@{}
foreach($id in @('drawing-canvas','layer-1-name','layer-1-rename','layer-opacity')){
 $control=[pscustomobject]@{Id=$id;Current=[pscustomobject]@{Id=$id}}
 $control.Current|Add-Member ScriptProperty HasKeyboardFocus {$this.Id -ceq [ImeSetupProbe]::Focused}
 $control|Add-Member ScriptMethod SetFocus {[ImeSetupProbe]::Focused=$this.Id}
 $control|Add-Member ScriptMethod GetCurrentPattern {
  param($pattern)
  if($pattern -eq [System.Windows.Automation.ValuePattern]::Pattern){return $script:valuePattern}
  if($pattern -eq [System.Windows.Automation.TextPattern]::Pattern){return $script:textPattern}
  throw 'Unexpected native editor pattern'
 }
 $controls[$id]=$control
}
$valuePattern=[pscustomobject]@{Current=[pscustomobject]@{}}
$valuePattern.Current|Add-Member ScriptProperty Value {[ImeSetupProbe]::Value}
$range=[pscustomobject]@{}
$range|Add-Member ScriptMethod GetText {param([int]$maximum)if($maximum -ne -1){throw 'Selection range was truncated'};if([ImeSetupProbe]::Selected){[ImeSetupProbe]::Value}else{'partial'}}
$textPattern=[pscustomobject]@{}
$textPattern|Add-Member ScriptMethod GetSelection {,$script:range}
function Control([string]$Id){if(!$controls.ContainsKey($Id)){throw "Unexpected IME control: $Id"};$controls[$Id]}
function Start-Sleep {}
function Wait-Until([scriptblock]$Condition,[string]$Message,[int]$Seconds){if(!(& $Condition)){throw $Message}}
function Model {@{state=@{layer_tools=@{rename_layer=$(if([ImeSetupProbe]::Rename){1}else{$null});editing_layer=@{id=1;label='Current ink'}}};windows_gpu_generation=11}}
function Capture-Desktop([string]$Name){
 if($Name -like '*-hiragana-preedit' -and [ImeSetupProbe]::Value -cne [ImeSetupProbe]::Expected){throw 'Preedit capture did not retain genuine text'}
 if($Name -like '*-converted-candidate' -and [ImeSetupProbe]::Value -notmatch '[\u3400-\u9fff]'){throw 'Candidate capture did not retain converted text'}
}
$review=[pscustomobject]@{Id=7};$registeredWindow=[pscustomobject]@{Handle=[IntPtr]71};$script:imeSequence=0
$body=@($ast.EndBlock.Statements|Where-Object {$_ -is [Management.Automation.Language.TryStatementAst]})[0].Body.Statements
$start=-1;$end=-1
for($i=0;$i -lt $body.Count;$i++){
 if($body[$i] -is [Management.Automation.Language.AssignmentStatementAst] -and $body[$i].Left.Extent.Text -ceq '$initial'){$start=$i}
 if($start -ge 0 -and $body[$i].FindAll({param($node)$node -is [Management.Automation.Language.CommandAst] -and $node.GetCommandName() -ceq 'Compose'},$true).Count){$end=$i;break}
}
if($start -lt 0 -or $end -lt $start){throw 'Missing actual initial IME setup path'}
$calls=@($ast.FindAll({param($node)$node -is [Management.Automation.Language.CommandAst] -and $node.GetCommandName() -ceq 'Compose'},$true))
if($calls.Count -ne 5){throw 'Genuine IME composition coverage changed'}
function Expect-Call($Call){
 [ImeSetupProbe]::Romaji=$Call.CommandElements[2].Value.ToUpperInvariant();[ImeSetupProbe]::Expected=$Call.CommandElements[3].Value
}
[ImeSetupProbe]::Reset();Expect-Call $calls[0]
foreach($statement in $body[$start..$end]){Invoke-Expression $statement.Extent.Text}
if([ImeSetupProbe]::ModeCalls -ne 1 -or [ImeSetupProbe]::Typed -cne 'NIHONN'){throw 'Initial IME setup did not prepare one mode before genuine romaji'}
$entry=Control 'layer-1-rename';$opacity=Control 'layer-opacity'
foreach($call in $calls|Select-Object -Skip 1){
 Expect-Call $call;Invoke-Expression $call.Extent.Text
 if([ImeSetupProbe]::Typed -cne [ImeSetupProbe]::Romaji -or [ImeSetupProbe]::ModeCalls -ne 1){throw 'A later composition changed IME mode or genuine key order'}
}
[ImeSetupProbe]::PartialSelection=$true;Expect-Call $calls[0];[ImeSetupProbe]::Typed='';$failure=$null
try{Invoke-Expression $calls[0].Extent.Text}catch{$failure=$_.Exception.Message}
if($failure -cne 'The owned editor did not select its full text before IME input' -or [ImeSetupProbe]::Typed -cne ''){throw 'Partial native selection reached romaji input'}
Write-Output 'IME setup passed the actual initial path, five composition callers and partial-selection rejection with owned API signatures; no desktop input.'
