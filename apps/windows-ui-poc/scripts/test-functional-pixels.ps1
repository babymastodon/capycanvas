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
@{passed=15;scope='PNG pixel algorithms; no screen capture or input'}|ConvertTo-Json|Set-Content -LiteralPath (Join-Path $Out 'pixel-tests.json') -Encoding UTF8
