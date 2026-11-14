$ErrorActionPreference='Stop'
$tokens=$null;$errors=$null
$ast=[Management.Automation.Language.Parser]::ParseFile((Join-Path $PSScriptRoot '../scripts/exercise-documents.ps1'),[ref]$tokens,[ref]$errors)
if($errors.Count){throw 'Document fixture parse failed'}
$helper=$ast.Find({param($node)$node -is [Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq 'Ink-ColorMatches'},$true)
. ([scriptblock]::Create($helper.Extent.Text))
foreach($case in @(
    @{space='Srgb';encoded=@(0,0,0);linear=@(0,0,0)},
    @{space='Srgb';encoded=@(.5,.01,-.5);linear=@(.21404114048223255,.0007739938080495357,-.21404114048223255)},
    @{space='DisplayP3';encoded=@(.5,1,0);linear=@(.21404114048223255,1,0)},
    @{space='AdobeRgb';encoded=@(.5,-.5,1);linear=@(.21775552814439456,-.21775552814439456,1)},
    @{space='ProPhoto';encoded=@(.5,-.5,.02);linear=@(.2871745887492587,-.2871745887492587,.00125)}
)){
    $definition=[pscustomobject]@{space=$case.space;rgba=@($case.encoded)+@(1)}
    if(!(Ink-ColorMatches $definition $case.linear)){throw "Encoded $($case.space) ink did not match linear RGB"}
    $different=@($case.linear);$different[1]+=.001
    if(Ink-ColorMatches $definition $different){throw 'Mismatched ink passed'}
}
$definition=[pscustomobject]@{space='Srgb';rgba=@(.18974828,.22091636,.17184409,1);linear_rgb=@(.03,.04,.025)}
if(!(Ink-ColorMatches $definition @(.03,.04,.025))){throw 'Preserved linear ink did not match'}
$definition.rgba=@(0,0,0,1)
if(Ink-ColorMatches $definition @(0,0,0)){throw 'Encoded values replaced the authored linear ink'}
$definition.rgba[3]=.5
if(Ink-ColorMatches $definition @(.03,.04,.025)){throw 'Translucent ink passed'}
$definition.rgba[3]=1;$definition.linear_rgb=@(.03,[double]::NaN,.025)
if(Ink-ColorMatches $definition @(.03,.04,.025)){throw 'Nonfinite ink passed'}
Write-Output 'Encoded and preserved linear ink checks passed'
& {
    Add-Type -AssemblyName UIAutomationClient,UIAutomationTypes
    Add-Type -TypeDefinition 'public static class CapyInkEntryKeys{public static bool Focused;public static int Keys,Commits;public static void Key(uint process,ushort key){Keys++;if(key==13&&Focused)Commits++;}}'
    $setInk=$ast.Find({param($node)$node -is [Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq 'Set-InkColor'},$true)
    if(!$setInk){throw 'Missing actual Set-InkColor'}
    . ([scriptblock]::Create($setInk.Extent.Text.Replace('[CapyRowPointer]','[CapyInkEntryKeys]')))
    $review=[pscustomobject]@{Id=7}
    $invoke=[pscustomobject]@{};$invoke|Add-Member ScriptMethod Invoke {}
    $value=[pscustomobject]@{};$value|Add-Member ScriptMethod SetValue {param($text)$script:inkValues+=,$text}
    $button=[pscustomobject]@{};$button|Add-Member ScriptMethod GetCurrentPattern {param($pattern)if($pattern -eq [System.Windows.Automation.ValuePattern]::Pattern){$value}else{$invoke}}
    $entry=[pscustomobject]@{}
    $entry|Add-Member ScriptMethod SetFocus {$script:inkFocusReads=0;[CapyInkEntryKeys]::Focused=$false}
    $entry|Add-Member ScriptMethod GetCurrentPattern {param($pattern)$value}
    $entry|Add-Member ScriptProperty Current {$script:inkFocusReads++;[CapyInkEntryKeys]::Focused=$script:inkFocusReads -ge 3;[pscustomobject]@{HasKeyboardFocus=[CapyInkEntryKeys]::Focused}}
    function Document-Panel([string]$Id){}
    function Choose-Item($Item){}
    function Wait-Until([scriptblock]$Condition,[string]$Message){for($poll=0;$poll -lt 6;$poll++){if(& $Condition){return}};throw $Message}
    function Control([string]$Value,$Within,[switch]$Arranged){if($Value -like '*-entry'){$entry}else{$button}}
    function Find-Id([string]$Id){
        if($Id -eq 'edit-color-form-0'){return [pscustomobject]@{Current=[pscustomobject]@{Name='Linear RGB'}}}
        if($Id -match '^edit-color-0-(\d)-entry$' -and [CapyInkEntryKeys]::Commits -le [int]$Matches[1]){return $entry}
    }
    function Model{[pscustomobject]@{layout=[pscustomobject]@{groups=@([pscustomobject]@{active='color';id=3})};state=[pscustomobject]@{colors=[pscustomobject]@{slot='foreground'}};paint_pair=[pscustomobject]@{definition=[pscustomobject]@{space='Srgb';rgba=@(.18974828,.22091636,.17184409,1);linear_rgb=@(.03,.04,.025)}}}}
    $script:inkValues=@()
    Set-InkColor @('.03','.04','.025')
    if([CapyInkEntryKeys]::Keys -ne 3 -or [CapyInkEntryKeys]::Commits -ne 3 -or ($script:inkValues -join ',') -cne '.03,.04,.025'){throw 'Controlled ink Enter reached an entry before its keyboard focus'}
    Write-Output 'Controlled ink entries take keyboard focus before Enter commits each value'
}
