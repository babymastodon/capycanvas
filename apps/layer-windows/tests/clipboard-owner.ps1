$ErrorActionPreference='Stop'
Add-Type -TypeDefinition @'
using System;
using System.Management.Automation.Runspaces;
public static class CapyClipboardOwnerTest {
 public static Runspace Owner;
 public static string Text;
 public static void SetText(string value){Owner=Runspace.DefaultRunspace;Text=value;}
 public static string GetText(){return Text;}
 public static void Copy(){
  if(Owner!=null && Owner.RunspaceStateInfo.State!=RunspaceState.Closed)
   throw new InvalidOperationException("EmptyClipboard blocked on the fixture's non-pumping owner");
  Text="#CA4B35";
 }
}
public static class CapyRowPointer {public static void Chord(uint id,ushort[] modifiers,int key){}}
public static class ClipboardLock {public static IntPtr GetOpenClipboardWindow(){return IntPtr.Zero;}}
'@ -ReferencedAssemblies ([Management.Automation.PowerShell].Assembly.Location)
function Load-Function([string]$Path,[string]$Name){
 $tokens=$null;$errors=$null
 $ast=[Management.Automation.Language.Parser]::ParseFile($Path,[ref]$tokens,[ref]$errors)
 if($errors.Count){throw "Parse failed: $Path"}
 $node=$ast.Find({param($n)$n -is [Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq $Name}.GetNewClosure(),$true)
 if(!$node){throw "Missing function: $Name"}
 $node.Extent.Text.Replace('[Windows.Forms.Clipboard]','[CapyClipboardOwnerTest]')
}
$scripts=Join-Path $PSScriptRoot '../scripts'
foreach($name in 'Sta','Wait-Scale','Wait-Until'){. ([scriptblock]::Create((Load-Function (Join-Path $scripts 'CapyUia.ps1') $name)))}
foreach($name in 'Paste','Clipboard-Text','Set-ClipboardText','Clipboard-Closed'){
 . ([scriptblock]::Create((Load-Function (Join-Path $scripts 'exercise-color-editor.ps1') $name)))
}
function Set-Clipboard([string]$Value){[CapyClipboardOwnerTest]::SetText($Value)}
function Get-Clipboard([switch]$Raw){[CapyClipboardOwnerTest]::GetText()}
function Control([string]$Id){New-Object PSObject|Add-Member ScriptMethod SetFocus {} -PassThru}
$review=[pscustomobject]@{Id=1}
foreach($text in 'rgb(202 75 53)',"native clipboard sentinel`r`nsecond line"){
 Paste $text
 if((Clipboard-Text) -cne $text){throw 'Clipboard text changed during transfer'}
 [CapyClipboardOwnerTest]::Copy()
 if((Clipboard-Text) -cne '#CA4B35'){throw 'App clipboard copy was not read'}
}
Write-Output 'Clipboard owner lifetime passed'
