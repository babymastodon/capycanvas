param([string]$Fixture=(Join-Path $PSScriptRoot '../scripts/exercise-hdr.ps1'))
$ErrorActionPreference='Stop'
$tokens=$null;$errors=$null
$ast=[Management.Automation.Language.Parser]::ParseFile($Fixture,[ref]$tokens,[ref]$errors)
if($errors.Count){throw ($errors|Out-String)}
$after=@($ast.FindAll({param($node)$node -is [Management.Automation.Language.AssignmentStatementAst] -and $node.Left.Extent.Text -ceq '$dialAfter'},$true))
if($after.Count -ne 1){throw 'Missing unique actual Dial reference assignment'}
$statements=$after[0].Parent.Statements;$index=-1
for($i=0;$i -lt $statements.Count;$i++){if($statements[$i].Extent.StartOffset -eq $after[0].Extent.StartOffset){$index=$i;break}}
if($index -lt 2 -or $statements[$index-2].Left.Extent.Text -cne '$settled' -or $statements[$index-1].PipelineElements[0].GetCommandName() -cne 'Wait-Until'){throw 'Unexpected actual Dial reference block'}
$reference=[scriptblock]::Create(($statements[($index-2)..$index]|ForEach-Object {$_.Extent.Text}) -join "`n")
$dialOwner=2;$dialArtwork='drawing-a';$dialEpoch=7;$dialCheckpoint=2
function Recipe([double]$Balance){[pscustomobject][ordered]@{balance=$Balance;contrast=1;exposure=-1;headroom=2;highlight_color=.3}}
$dialBefore=Recipe 0|ConvertTo-Json -Compress
$expected=Recipe .25|ConvertTo-Json -Compress
function Start-Sleep([int]$Milliseconds){$script:clock+=$Milliseconds}
function Wait-Until([scriptblock]$Test,[string]$Message){
 for($attempt=0;$attempt -lt 12;$attempt++){if(& $Test){return};$script:clock+=50}
 throw $Message
}
function Model{
 $committed=$script:clock -ge 300 -or $script:scenario -eq 'already-committed'
 $owner=2;$artwork='drawing-a';$epoch=7;$checkpoint=if($committed){5}else{2};$balance=if($committed){.25}else{-.01}
 if($script:scenario -eq 'no-checkpoint'){$checkpoint=$null}
 if($script:scenario -eq 'never-committed'){$checkpoint=2}
 if($script:scenario -eq 'no-change'){$balance=0}
 if(!$committed -and $script:scenario -eq 'wrong-owner'){$owner=3;$checkpoint=9}
 if(!$committed -and $script:scenario -eq 'wrong-artwork'){$artwork='drawing-b';$checkpoint=9}
 if(!$committed -and $script:scenario -eq 'wrong-epoch'){$epoch=8;$checkpoint=9}
 [pscustomobject]@{
  windows_tabs=[pscustomobject]@{selected=$owner;session_stamps=@([pscustomobject]@{id=$owner;stamp=[pscustomobject]@{artwork=$artwork;epoch=$epoch;checkpoint=$checkpoint;revision=$script:clock}})}
  windows_proof_form=[pscustomobject]@{rendition=(Recipe $balance)}
  state=[pscustomobject]@{revision=$script:clock;commands=@([pscustomobject]@{id='undo';enabled=$true},[pscustomobject]@{id='redo';enabled=$true})}
 }
}
$checks=0
foreach($case in @('delayed-commit','wrong-owner','wrong-artwork','wrong-epoch','already-committed','never-committed','no-checkpoint','no-change')){
 $script:scenario=$case;$script:clock=0;$dialAfter=$null;$failure=$null
 try{. $reference}catch{$failure=$_.Exception.Message}
 if($case -in @('never-committed','no-checkpoint','no-change')){
  if($failure -cne 'Dial release did not commit'){throw "$case admitted a reference without the requested committed edit"}
 }else{
  if($failure){throw "$case failed: $failure"}
  if($dialAfter -cne $expected){throw 'Dial reference came from the pre-commit checkpoint'}
  if($case -eq 'already-committed' -and $script:clock -ne 0){throw 'Committed Dial reference still depends on a timing heuristic'}
 }
 $checks++
}
Write-Output "HDR Dial committed reference: $checks passed"
