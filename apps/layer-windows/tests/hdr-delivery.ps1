param([string]$Fixture=(Join-Path $PSScriptRoot '../scripts/exercise-hdr.ps1'))
$ErrorActionPreference='Stop'
Add-Type -AssemblyName UIAutomationClient,UIAutomationTypes
$tokens=$null;$errors=$null
$ast=[Management.Automation.Language.Parser]::ParseFile($Fixture,[ref]$tokens,[ref]$errors)
if(@($errors).Count){throw ($errors|Out-String)}
$definition=@($ast.FindAll({param($node)$node -is [Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq 'Delivery'},$true))
if($definition.Count -ne 1){throw 'Missing actual HDR delivery function'}
. ([scriptblock]::Create($definition[0].Extent.Text))
foreach($name in @('Control','Button')){
    $helper=@($ast.FindAll({param($node)$node -is [Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq $name},$true))
    if($helper.Count -ne 1){throw "Missing actual HDR $name helper"}
    $body=$helper[0].Extent.Text;if($name -eq 'Button'){$body=$body.Replace('function Button(','function Invoke-VisibleButton(')}
    . ([scriptblock]::Create($body))
}
$calls=@($ast.FindAll({param($node)$node -is [Management.Automation.Language.CommandAst] -and $node.GetCommandName() -eq 'Delivery'},$true))
if($calls.Count -ne 13){throw 'HDR delivery coverage changed'}
$run='C:\isolated-hdr-delivery'
$copy=[pscustomobject]@{format_png='Bild PNG';format_pq='Bild HDR PQ';format_exr='Bild OpenEXR';format_jpeg_hdr='Bild HDR JPEG';format_avif_hdr='Bild HDR AVIF';format_jpeg_hdr_clipped='Bild HDR JPEG begrenzt';format_avif_hdr_clipped='Bild HDR AVIF begrenzt';white_background='Weisser Hintergrund';common=@{reset='Zuruecksetzen'};preview='Ausgabe ansehen';export='Exportieren'}
$formats=@{format_png='Png';format_pq='PngHdr';format_exr='Exr';format_jpeg_hdr='JpegHdr';format_avif_hdr='AvifHdr';format_jpeg_hdr_clipped='JpegHdrMapped';format_avif_hdr_clipped='AvifHdrMapped'}
$resetPattern=[pscustomobject]@{}
$resetPattern|Add-Member ScriptMethod Invoke {$script:resetInvokes++;$script:document.serial++}
$reset=[pscustomobject]@{Current=[pscustomobject]@{Name=$copy.common.reset;ControlType=[System.Windows.Automation.ControlType]::Button;ProcessId=77;IsOffscreen=$true}}
$reset|Add-Member ScriptMethod GetCurrentPattern {param($pattern)if($pattern -ne [System.Windows.Automation.InvokePattern]::Pattern){throw 'Reset requires InvokePattern'};$script:resetPattern}
$root=[pscustomobject]@{Current=[pscustomobject]@{ProcessId=77}}
$root|Add-Member ScriptMethod FindFirst {
    param($scope,$condition)
    if($scope -ne [System.Windows.Automation.TreeScope]::Descendants -or $condition -isnot [System.Windows.Automation.AndCondition]){throw 'Reset lookup escaped the owned descendant query'}
    $parts=@($condition.GetConditions())
    $name=@($parts|Where-Object {$_.Property -eq [System.Windows.Automation.AutomationElement]::NameProperty})
    $type=@($parts|Where-Object {$_.Property -eq [System.Windows.Automation.AutomationElement]::ControlTypeProperty})
    if($parts.Count -ne 2 -or $name.Count -ne 1 -or $type.Count -ne 1){throw 'Reset query lost its name/type conditions'}
    $script:resetQueries++
    if($name[0].Value -cne $script:reset.Current.Name -or $type[0].Value -ne $script:reset.Current.ControlType.Id){return $null}
    if($script:reset.Current.ProcessId -ne $this.Current.ProcessId){throw 'Reset is outside the owned root'}
    $script:reset
}
function Find([string]$Value,[switch]$Name,$Type,$Within=$root){
    $current=$script:reset.Current
    if($Name -and $current.Name -ceq $Value -and $current.ControlType -eq $Type -and $current.ProcessId -eq $Within.Current.ProcessId -and !$current.IsOffscreen){$script:reset}
}
function Command([string]$Id,[string]$Menu){
    if($Id -cne 'export_document' -or $Menu -cne 'File'){throw 'Unexpected delivery command'}
    $script:document=[pscustomobject]@{stage='options';serial=1;details=@{form=@{copy=$copy};recipe=@{format='Png'}}}
}
function Model {@{windows_document=$script:document}}
function Wait-Until([scriptblock]$Condition,[string]$Message,[int]$Seconds){if(!(& $Condition)){throw $Message}}
function Select-Choice([string]$Id,[string]$Name){
    $wanted=if($Id -ceq 'export-format'){$copy.($script:expectedKey)}elseif($Id -ceq 'export-background'){$copy.white_background}else{throw "Unexpected delivery choice: $Id"}
    if(!$wanted -or $Name -cne $wanted){throw "Missing HDR control: $Name"}
    if($Id -ceq 'export-format'){$script:selected=$formats[$script:expectedKey]}
    $script:choices+=@($Id)
}
function Button([string]$Name){
    if($Name -ceq $copy.common.reset){Invoke-VisibleButton $Name;return}
    if($Name -ceq $copy.preview){$script:previews++;$script:document.stage='preview';$script:document.details.recipe.format=if($script:wrongFormat){$script:wrongFormat}else{$script:selected};return}
    if($Name -ceq $copy.export){$script:exports++;return}
    throw "Unexpected delivery button: $Name"
}
function Picker([string]$Name){if($Name -cne 'Save As'){throw 'Unexpected delivery picker'}}
function Path-In-Picker([string]$Path){$script:exported=$Path}
function Idle {}
function Test-Path([string]$LiteralPath){$LiteralPath -ceq $script:exported}
function Get-FileHash([string]$LiteralPath,[string]$Algorithm){
    if($LiteralPath -cne $script:exported -or $Algorithm -cne 'SHA256'){throw 'Delivery hash ownership changed'}
    @{Hash='exact-delivery-hash'}
}
function Reset-Delivery([string]$Key){
    $script:reset.Current.Name=$copy.common.reset;$script:resetQueries=0;$script:resetInvokes=0;$script:expectedKey=$Key;$script:selected=$null;$script:wrongFormat=$null;$script:previews=0;$script:exports=0;$script:choices=@();$script:exported=$null
}
$keys=@('format_png','format_pq','format_exr','format_jpeg_hdr','format_avif_hdr','format_jpeg_hdr_clipped','format_avif_hdr_clipped','format_exr','format_exr','format_png','format_png','format_exr','format_exr')
foreach($index in 0..($calls.Count-1)){
    $copy.common.reset=if($index -eq 0){'Reset'}else{'Zuruecksetzen'}
    Reset-Delivery $keys[$index]
    $hash=Invoke-Expression $calls[$index].Extent.Text
    if($hash -cne 'exact-delivery-hash' -or $script:previews -ne 1 -or $script:exports -ne 1){throw 'Delivery did not return the exact exported file hash'}
    if($script:resetQueries -ne 1 -or $script:resetInvokes -ne 1){throw 'Delivery did not invoke the owned offscreen Reset exactly once'}
    $background=$keys[$index] -in @('format_jpeg_hdr','format_jpeg_hdr_clipped')
    if(@($script:choices|Where-Object {$_ -ceq 'export-background'}).Count -ne [int]$background){throw 'HDR JPEG background selection changed'}
}
foreach($key in @('format_jpeg_hdr_clipped','format_avif_hdr_clipped','format_png')){
    Reset-Delivery $key;$script:wrongFormat=if($key -eq 'format_png'){'PngHdr'}elseif($key -eq 'format_jpeg_hdr_clipped'){'JpegHdr'}else{'AvifHdr'};$failure=$null
    try{Delivery 'wrong-format' $key}catch{$failure=$_.Exception.Message}
    if($failure -cne 'HDR export preview selected another format' -or $script:exports -ne 0){throw 'Wrong preview format reached Export'}
}
Reset-Delivery 'format_png'
$saved=$copy.format_png;$copy.format_png=$null;$failure=$null
try{Delivery 'missing-copy.png' 'format_png'}catch{$failure=$_.Exception.Message}finally{$copy.format_png=$saved}
if($failure -notlike 'Missing HDR control:*' -or $script:previews -ne 0){throw 'Missing localized format copy reached Preview Output'}
Write-Output 'HDR delivery passed 13 actual callers, owned offscreen localized Reset, seven localized format keys, exact hash return, existing preview recipe identity and missing/wrong-format controls; no window or GPU.'

$guard=@($ast.FindAll({param($node)$node -is [Management.Automation.Language.IfStatementAst] -and $node.Extent.Text -match "throw '(Window|Drawing) close cancellation lost drawings'"},$true))
if($guard.Count -ne 1){throw 'Missing actual HDR drawing-close assertion'}
$statements=@($guard[0].Parent.Statements);$end=[array]::IndexOf($statements,$guard[0]);$start=$end-1
while($start -ge 0 -and $statements[$start].Extent.Text -notmatch '^\$count='){$start--}
if($start -lt 0){throw 'Missing HDR drawing-close baseline'}
Add-Type -TypeDefinition 'public static class CapyWindowApi { public static bool Closed; public static bool PostMessage(object w,int m,object a,object b){Closed=true;return true;} }'
$drawingWindow=[IntPtr]::Zero;$script:closePending=$false;$script:closeCancelled=$false
function Model {@{windows_tabs=@{tabs=@(@{id=1},@{id=2})}}}
function Command([string]$Id,[string]$Menu){
    if($Id -cne 'close_document' -or $Menu -cne 'File'){throw 'HDR cancellation requires explicit Close Drawing'}
    $script:closePending=$true
}
function Button([string]$Name){
    if([CapyWindowApi]::Closed){throw 'Owned review exited with code 0: Missing HDR control: Cancel'}
    if($Name -cne 'Cancel' -or !$script:closePending){throw 'Missing HDR control: Cancel'}
    $script:closePending=$false;$script:closeCancelled=$true
}
function Idle {if($script:closePending){throw 'HDR close decision remains pending'}}
$closePath=($statements[$start..$end]|ForEach-Object {$_.Extent.Text}) -join "`n"
Invoke-Expression $closePath
if(!$script:closeCancelled -or [CapyWindowApi]::Closed){throw 'HDR did not cancel the explicit drawing close'}
Write-Output 'HDR cancellation keeps both drawings through explicit Close Drawing, without quitting the recoverable session.'
