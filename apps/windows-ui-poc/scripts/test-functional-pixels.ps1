param([Parameter(Mandatory)][string]$Out)
$ErrorActionPreference='Stop'
Add-Type -AssemblyName System.Drawing
if(!('PocFunctionalPixels' -as [type])){Add-Type -Path (Join-Path $PSScriptRoot 'FunctionalNative.cs') -ReferencedAssemblies System.Drawing}
[IO.Directory]::CreateDirectory($Out)|Out-Null
function Assert([bool]$Value,[string]$Message){if(!$Value){throw $Message}}
$before=Join-Path $Out 'before.png';$after=Join-Path $Out 'after.png';$marker=Join-Path $Out 'marker.png';$crop=Join-Path $Out 'crop.png'
$bitmap=[Drawing.Bitmap]::new(128,64)
try{
    $graphics=[Drawing.Graphics]::FromImage($bitmap)
    try{$graphics.Clear([Drawing.Color]::Black)}finally{$graphics.Dispose()}
    $bitmap.Save($before,[Drawing.Imaging.ImageFormat]::Png)
    foreach($x in 8..11){foreach($y in 8..11){$bitmap.SetPixel($x,$y,[Drawing.Color]::Red)}}
    foreach($x in 60..69){foreach($y in 20..29){$bitmap.SetPixel($x,$y,[Drawing.Color]::White)}}
    $bitmap.Save($after,[Drawing.Imaging.ImageFormat]::Png)
    $diff=[PocFunctionalPixels]::Difference($before,$after,8,8,4,4)
    Assert (($diff -join ',') -eq '60,20,10,10,100') 'Candidate bbox included the excluded text field or lost changed pixels'
    Assert ($null -eq [PocFunctionalPixels]::Difference($before,$after,0,0,128,64)) 'An excluded whole frame produced a candidate'
    [PocFunctionalPixels]::Crop($after,$crop,60,20,10,10)
    $cropped=[Drawing.Bitmap]::new($crop)
    try{Assert ($cropped.Width -eq 10 -and $cropped.Height -eq 10) 'Candidate crop dimensions changed'}finally{$cropped.Dispose()}
    $graphics=[Drawing.Graphics]::FromImage($bitmap)
    try{$graphics.Clear([Drawing.Color]::Black)}finally{$graphics.Dispose()}
    foreach($x in 3..100){foreach($y in 3..8){$bitmap.SetPixel($x,$y,[Drawing.Color]::FromArgb(17,238,68))}}
    for($cell=0;$cell -lt 24;$cell++){for($x=4+$cell*4;$x -lt 8+$cell*4;$x++){foreach($y in 4..7){$bitmap.SetPixel($x,$y,$(if($cell%2){[Drawing.Color]::White}else{[Drawing.Color]::Black}))}}}
    $bitmap.SetPixel(110,20,[Drawing.Color]::Magenta);$bitmap.SetPixel(110,21,[Drawing.Color]::Magenta)
    $bitmap.Save($marker,[Drawing.Imaging.ImageFormat]::Png)
    Assert ([PocFunctionalPixels]::Marker($marker,4,4)) 'Exact framed 4px marker was rejected'
    Assert (([PocFunctionalPixels]::ColorRuns($marker,110,255,0,255) -join ',') -eq '20,2') 'Exact 2px probe run changed'
    $bitmap.SetPixel(5,5,[Drawing.Color]::Gray);$bitmap.Save($marker,[Drawing.Imaging.ImageFormat]::Png)
    Assert (![PocFunctionalPixels]::Marker($marker,4,4)) 'A nonuniform marker cell passed'
    $bitmap.SetPixel(5,5,[Drawing.Color]::Black);$bitmap.SetPixel(3,4,[Drawing.Color]::Black);$bitmap.Save($marker,[Drawing.Imaging.ImageFormat]::Png)
    Assert (![PocFunctionalPixels]::Marker($marker,4,4)) 'A missing guard pixel passed'
}finally{$bitmap.Dispose()}
Assert ([PocFunctionalPixels]::EdgeDistance([double[]]@(0,0,100,100),[double[]]@(49,49,2,2)) -eq 49) 'A huge surrounding difference falsely had zero candidate edge distance'
Assert ([PocFunctionalPixels]::EdgeDistance([double[]]@(12,20,20,20),[double[]]@(10,20,2,10)) -eq 0) 'A touching candidate edge was rejected'
$segBefore=Join-Path $Out 'segmented-before.png';$segAfter=Join-Path $Out 'segmented-after.png';$rawImage=Join-Path $Out 'segmented-raw-diff.png'
$masks=[int[]]@(160,40,30,20,3,3,98,6,3,147,98,6);$caret=[double[]]@(180,50,1,10)
$bitmap=[Drawing.Bitmap]::new(256,160)
try{
    $graphics=[Drawing.Graphics]::FromImage($bitmap)
    try{$graphics.Clear([Drawing.Color]::Black)}finally{$graphics.Dispose()}
    $bitmap.Save($segBefore,[Drawing.Imaging.ImageFormat]::Png)
    $graphics=[Drawing.Graphics]::FromImage($bitmap)
    try{
        $graphics.FillRectangle([Drawing.Brushes]::White,3,3,98,6);$graphics.FillRectangle([Drawing.Brushes]::White,3,147,98,6)
        $graphics.FillRectangle([Drawing.Brushes]::Red,160,40,30,20);$graphics.FillRectangle([Drawing.Brushes]::Gray,10,100,70,20);$graphics.FillRectangle([Drawing.Brushes]::Green,170,70,50,25)
    }finally{$graphics.Dispose()}
    $bitmap.Save($segAfter,[Drawing.Imaging.ImageFormat]::Png)
    $regions=@([PocFunctionalPixels]::Regions($segBefore,$segAfter,$masks))
    Assert ($regions.Count -eq 2) 'The two markers or the text field escaped their masks'
    $candidates=@([PocFunctionalPixels]::CandidateRegions([PocFunctionalPixels+Region[]]$regions,$caret,10))
    Assert ($candidates.Count -eq 1 -and $regions[$candidates[0]].X -eq 170) 'Unrelated list changes displaced the actual near-caret candidate'
    $raw=[PocFunctionalPixels]::Difference($segBefore,$segAfter,0,0,0,0)
    Assert ($raw[4] -gt ($regions|Measure-Object ChangedPixels -Sum).Sum) 'Raw differences lost the masked diagnostic changes'
    [PocFunctionalPixels]::DifferenceImage($segBefore,$segAfter,$rawImage)
    $diffImage=[Drawing.Bitmap]::new($rawImage)
    try{Assert ($diffImage.Width -eq 256 -and $diffImage.Height -eq 160) 'Raw difference image changed the screen geometry'}finally{$diffImage.Dispose()}
    $graphics=[Drawing.Graphics]::FromImage($bitmap)
    try{$graphics.FillRectangle([Drawing.Brushes]::Blue,195,40,35,20)}finally{$graphics.Dispose()}
    $bitmap.Save($segAfter,[Drawing.Imaging.ImageFormat]::Png)
    $regions=@([PocFunctionalPixels]::Regions($segBefore,$segAfter,$masks));$candidates=@([PocFunctionalPixels]::CandidateRegions([PocFunctionalPixels+Region[]]$regions,$caret,10))
    Assert ($candidates.Count -eq 2) 'Two plausible near-caret components were silently ranked instead of remaining ambiguous'
    $graphics=[Drawing.Graphics]::FromImage($bitmap)
    try{$graphics.FillRectangle([Drawing.Brushes]::Black,170,70,50,25);$graphics.FillRectangle([Drawing.Brushes]::Black,195,40,35,20);$graphics.FillRectangle([Drawing.Brushes]::Blue,195,40,35,1)}finally{$graphics.Dispose()}
    $bitmap.Save($segAfter,[Drawing.Imaging.ImageFormat]::Png)
    $regions=@([PocFunctionalPixels]::Regions($segBefore,$segAfter,$masks));$candidates=@([PocFunctionalPixels]::CandidateRegions([PocFunctionalPixels+Region[]]$regions,$caret,10))
    Assert ($candidates.Count -eq 0) 'A missing candidate was replaced with a tiny nearby focus border'
}finally{$bitmap.Dispose()}
Assert (![PocFunctionalNative]::BoundsVisible([double[]]@(100,2000,40,44),[double[]]@(100,100,304,440),$false)) 'A far row was treated as visible without intersecting its viewport'
Assert ([PocFunctionalNative]::BoundsVisible([double[]]@(100,90,40,44),[double[]]@(100,100,304,440),$false) -and ![PocFunctionalNative]::BoundsVisible([double[]]@(100,90,40,44),[double[]]@(100,100,304,440),$true)) 'Partially clipped bounds were confused with a fully visible row'
Assert ([PocFunctionalNative]::BoundsVisible([double[]]@(100,100,304,44),[double[]]@(100,100,304,440),$true)) 'An actual contained row was rejected'
Assert (![PocFunctionalNative]::BoundsVisible([double[]]@(100,100,0,44),[double[]]@(100,100,304,440),$false) -and ![PocFunctionalNative]::BoundsVisible([double[]]@([double]::NaN,100,40,44),[double[]]@(100,100,304,440),$false)) 'Empty or nonfinite UIA bounds passed visibility checks'
$tokens=$null;$parseErrors=$null;$ast=[Management.Automation.Language.Parser]::ParseFile((Join-Path $PSScriptRoot 'check-functional.ps1'),[ref]$tokens,[ref]$parseErrors)
if($parseErrors.Count){throw ($parseErrors|Out-String)}
foreach($name in @('Json-Lines','Text-LayoutEvidence','Read-ImeValue')){$definition=$ast.Find({param($node)$node -is [Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq $name},$true);Invoke-Expression $definition.Extent.Text}
$testSpec=[pscustomobject]@{labels=@(0..6|ForEach-Object {"label$_"});paragraph='wrapped paragraph'}
$trace=@($testSpec.labels|ForEach-Object {[pscustomobject]@{kind='text_layout';text=$_;wrap=$false;missing_glyphs=0;line_breaks=@($_.Length)}})
$trace+=@([pscustomobject]@{kind='text_layout';text=$testSpec.paragraph;wrap=$true;missing_glyphs=0;line_breaks=@(8,17)})
$evidence=Text-LayoutEvidence $trace $testSpec
Assert ($evidence.status -eq 'observed' -and $evidence.missing_glyphs -eq 0 -and $evidence.labels.Count -eq 7 -and ($evidence.paragraph[0].line_breaks -join ',') -eq '8,17' -and $evidence.thai_word_boundaries.StartsWith('unverified')) 'Complete glyph evidence lost real wrap offsets or claimed Thai boundaries were proven'
$trace[0].missing_glyphs=1;$evidence=Text-LayoutEvidence $trace $testSpec
Assert ($evidence.status -eq 'fail' -and $evidence.missing_glyphs -eq 1) 'An actual missing glyph did not fail text evidence'
$trace[0].missing_glyphs=0;$evidence=Text-LayoutEvidence @($trace|Where-Object text -ne 'label6') $testSpec
Assert ($evidence.status -eq 'unverified' -and $evidence.missing_text_records -contains 'label6') 'Missing text records falsely proved zero missing glyphs'
$unicodeSpec=[pscustomobject]@{labels=@(0x65e5,0x56fe,0x5716,0xd55c,0x0e0a,0x1edb,0x0421|ForEach-Object {[string][char]$_});paragraph=(-join [char[]]@(0x0e44,0x0e17,0x0e22,0x65e5,0x672c))}
$unicodeLines=@($unicodeSpec.labels|ForEach-Object {@{kind='text_layout';text=$_;wrap=$false;missing_glyphs=0;line_breaks=@([Text.Encoding]::UTF8.GetByteCount($_))}|ConvertTo-Json -Compress})
$unicodeLines+=@(@{kind='text_layout';text=$unicodeSpec.paragraph;wrap=$true;missing_glyphs=0;line_breaks=@(6,15)}|ConvertTo-Json -Compress)
$unicodePath=Join-Path $Out 'utf8-no-bom-text-layout.jsonl';[IO.File]::WriteAllText($unicodePath,($unicodeLines -join "`n"),[Text.UTF8Encoding]::new($false))
$decoded=@(Json-Lines $unicodePath);$evidence=Text-LayoutEvidence $decoded $unicodeSpec
$unicodeBytes=[IO.File]::ReadAllBytes($unicodePath)
Assert ($unicodeBytes[0] -eq 0x7b -and @($unicodeBytes|Where-Object {$_ -ge 128}).Count -gt 0 -and $decoded.Count -eq 8 -and $decoded[0].text -ceq $unicodeSpec.labels[0] -and $evidence.status -eq 'observed' -and $evidence.missing_text_records.Count -eq 0 -and $evidence.paragraph[0].text -ceq $unicodeSpec.paragraph) 'UTF-8 JSONL without a BOM lost literal multilingual labels or Thai paragraph matches'
Add-Type -AssemblyName UIAutomationClient,UIAutomationTypes
$uiaPath=Join-Path $PSScriptRoot '../../layer-windows/scripts/CapyUia.ps1';$uiaAst=[Management.Automation.Language.Parser]::ParseFile($uiaPath,[ref]$tokens,[ref]$parseErrors)
if($parseErrors.Count){throw ($parseErrors|Out-String)}
foreach($name in @('Wait-Scale','Wait-Until')){$definition=$uiaAst.Find({param($node)$node -is [Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq $name},$true);Invoke-Expression $definition.Extent.Text}
$gates=@($ast.FindAll({param($node)$node -is [Management.Automation.Language.CommandAst] -and $node.GetCommandName() -eq 'Read-ImeValue'},$true))
if($gates.Count -ne 3){throw 'Expected the actual preedit, candidate and committed IME gates'}
$values=@((-join [char[]]@(0x306b,0x307b,0x3093,0x3054)),(-join [char[]]@(0x65e5,0x672c,0x8a9e)),(-join [char[]]@(0x65e5,0x672c,0x8a9e)))
$script:CapyEach=$null;$review=$null
for($gate=0;$gate -lt 3;$gate++){
    $provider=@{reads=0;text=$values[$gate]};$accept=$gates[$gate].CommandElements[2].ScriptBlock.GetScriptBlock();$message=$gates[$gate].CommandElements[3].Value
    $value=Read-ImeValue {$provider.reads++;if($provider.reads -gt 1){throw 'Provider was queried after its accepted IME value'};$provider.text} $accept $message
    Assert ($value -ceq $provider.text -and $provider.reads -eq 1) "IME gate $gate queried the provider again instead of retaining the exact accepted value"
}
@{passed=26;scope='PNG, UIA geometry, retained trace and IME accepted-value evidence; no screen capture or input'}|ConvertTo-Json|Set-Content -LiteralPath (Join-Path $Out 'pixel-tests.json') -Encoding UTF8
