param([string]$Fixture=(Join-Path $PSScriptRoot '../scripts/exercise-documents.ps1'),[string]$Output)
$ErrorActionPreference='Stop'
. (Join-Path $PSScriptRoot '../scripts/ComposedFrames.ps1')
$tokens=$null;$errors=$null
$ast=[Management.Automation.Language.Parser]::ParseFile($Fixture,[ref]$tokens,[ref]$errors)
if(@($errors).Count){throw ($errors|Out-String)}
$definition=@($ast.FindAll({param($node)$node -is [Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq 'Retention-Frames'},$true))
if($definition.Count -ne 1){throw 'Missing actual document retention sampler'}
. ([scriptblock]::Create($definition[0].Extent.Text))
if(!$Output){$Output=Join-Path $PSScriptRoot ('../../../artifacts/windows/tests/document-retention-'+[guid]::NewGuid().ToString('N'))}
$run=Join-Path $Output ([guid]::NewGuid().ToString('N'))
$controls=@{}
foreach($pair in @(@('export-preset-name',1),@('action',2))){
    $control=[pscustomobject]@{runtime=$pair[1];Current=[pscustomobject]@{runtime=$pair[1];BoundingRectangle=[Drawing.RectangleF]::new(519,705,180,93)}}
    $control.Current|Add-Member ScriptProperty HasKeyboardFocus {$script:focused -eq [string]$this.runtime}
    $control|Add-Member ScriptMethod SetFocus {$script:focused=[string]$this.runtime}
    $control|Add-Member ScriptMethod GetRuntimeId {[int[]]@($this.runtime)}
    $controls[$pair[0]]=$control
}
function Owned([string]$Id){$controls[$Id]}
function Wait-Until([scriptblock]$Condition,[string]$Message){if(!(& $Condition)){throw $Message}}
function Model {@{windows_document=@{id=3;serial=$script:serial}}}
function Find-Id([string]$Id){$Id -eq 'document-workflow'}
function Retention-State([string]$FieldId){
    $state=$script:retained.Clone();$state.serial=$script:serial;$state.focused=$script:focused;$state.focus=$script:focused -eq '1';$state
}
function Sample-Frames($Area,[scriptblock]$Action){
    if(($Area.Left,$Area.Top,$Area.Width,$Area.Height -join ',') -cne '519,705,90,93'){throw 'Retention field crop changed'}
    $frames=[Collections.Generic.List[Drawing.Bitmap]]::new()
    for($i=0;$i -lt 6;$i++){
        & $Action $i|Out-Null
        $image=[Drawing.Bitmap]::new($Area.Width,$Area.Height);$graphics=[Drawing.Graphics]::FromImage($image)
        try{
            $graphics.Clear([Drawing.Color]::FromArgb(71,71,72))
            foreach($x in @(19,31,43,55)){$graphics.FillRectangle([Drawing.Brushes]::White,$x,58,6,18)}
            if($script:focused -eq '1'){$graphics.FillRectangle([Drawing.Brushes]::Silver,17,54,2,29)}
            if($script:pixelDamage){$graphics.FillRectangle([Drawing.Brushes]::Black,19,58,6,18)}
        }finally{$graphics.Dispose()}
        $frames.Add($image)
    }
    @{frames=$frames;times=@()}
}
$checks=0
function Check-Retention([string]$Name,[bool]$Button,[string]$Destination,[hashtable]$Changed=@{},[switch]$Damage,[string[]]$Expected=@()){
    $script:focused=$null;$script:serial=1;$script:pixelDamage=$false
    $script:retained=@{request=3;kind='export';stage='options';dialog='dialog';field='1';value='Retention draft preserved text';scroll=100}
    $target=if($Button){$controls.action}else{$null}
    $result=Retention-Frames $Name {
        $script:serial++;$script:focused=$Destination;$script:pixelDamage=[bool]$Damage
        foreach($key in $Changed.Keys){$script:retained[$key]=$Changed[$key]}
    } -FocusTarget $target -Acknowledged {param($view)$view.serial -eq 2}
    if(($result.failures -join ';') -cne ($Expected -join ';')){
        throw "$Name caret/focus baseline: expected '$($Expected -join ';')', observed '$($result.failures -join ';')'"
    }
    $baseline=if($Button){'2'}else{'1'}
    if($result.before.focused -cne $baseline){throw "$Name did not establish its permitted focus before the pixel baseline"}
    if(($result.area -join ',') -cne '519,705,90,93'){throw "$Name changed its field crop"}
    $script:checks++
}
try{
    foreach($action in @('save','update','reset')){Check-Retention $action $true '2'}
    Check-Retention 'get' $false '1'
    Check-Retention 'lost-text' $true '2' -Damage -Expected 'unchanged draft pixels disappeared or moved'
    foreach($key in @('request','kind','stage','dialog','field','value','scroll')){
        $changed=@{};$changed[$key]=if($key -in @('request','scroll')){999}else{'changed'}
        Check-Retention ("changed-"+$key) $true '2' -Changed $changed -Expected ("retained "+$key+" changed")
    }
    Check-Retention 'unrelated-focus' $true '3' -Expected 'focus left the retained field and invoked control'
    Write-Output "Document retention passed $checks cases using the actual sampler and pixel envelope: permitted focus, lost text, retained identity/value/scroll and unrelated focus; no window or GPU."
}finally{if(Test-Path -LiteralPath $run){Remove-Item -LiteralPath $run -Recurse -Force}}

& {
    $owned=$ast.Find({param($node)$node -is [Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq 'Owned'},$true)
    if(!$owned){throw 'Missing actual document owned-control wait'}
    . ([scriptblock]::Create($owned.Extent.Text))
    $selections=@($ast.FindAll({param($node)
        $node -is [Management.Automation.Language.CommandAst] -and $node.GetCommandName() -eq 'Combo-Select' -and
        ($node.Extent.Text -like '*WebP*' -or $node.Extent.Text -like '*JPEG image*')
    },$true))
    if($selections.Count -ne 2){throw 'Missing actual default export format selections'}
    $root=[pscustomobject]@{owner='document'}
    function Find([string]$Id,$Within){
        if($Id -cne 'export-format' -or $Within -ne $root){throw 'Export looked outside its owned Format control'}
        if($script:formatPublished){$script:formatControl}
    }
    function Find-Id([string]$Id){Find $Id -Within $root}
    function Wait-Until([scriptblock]$Condition,[string]$Message){
        if(& $Condition){throw 'Export control scenario did not begin before native publication'}
        $script:formatPublished=$true
        if(!(& $Condition)){throw $Message}
    }
    function Combo-Select($Combo,[scriptblock]$Pick){
        if(!$Combo){throw 'Export selected Format before its native control was published'}
        if($Combo -ne $script:formatControl){throw 'Export selected another native control'}
        $script:formatChoice=@(@('WebP lossless','JPEG image')|ForEach-Object {[pscustomobject]@{Current=[pscustomobject]@{Name=$_}}}|Where-Object $Pick)
    }
    foreach($selection in $selections){
        $script:formatPublished=$false;$script:formatControl=[pscustomobject]@{id='export-format'};$script:formatChoice=@();$format=$null
        & ([scriptblock]::Create($selection.Extent.Text))
        if(!$script:formatPublished -or $script:formatChoice.Count -ne 1){throw 'Export did not wait for and select its native format'}
        $expected=if($selection.Extent.Text -like '*WebP*'){'WebP lossless'}else{'JPEG image'}
        if($script:formatChoice[0].Current.Name -cne $expected){throw 'Export format selection changed'}
    }
    'Default export format selections wait for native publication: passed'
}
