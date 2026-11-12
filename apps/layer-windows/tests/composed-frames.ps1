$ErrorActionPreference='Stop'
. (Join-Path $PSScriptRoot '../scripts/ComposedFrames.ps1')
$baseline=[Drawing.Bitmap]::new(4,4);$changed=$null
try{
    $changed=[Drawing.Bitmap]::new(4,4)
    for($y=0;$y -lt 4;$y++){for($x=0;$x -lt 4;$x++){
        $baseline.SetPixel($x,$y,[Drawing.Color]::Black)
        $changed.SetPixel($x,$y,[Drawing.Color]::Black)
    }}
    $envelope=[TabFrames+Envelope]::new($baseline,$baseline)
    if($envelope.Difference($changed) -ne 0){throw 'Composed frames changed an identical black image'}
    $changed.SetPixel(1,1,[Drawing.Color]::White)
    if($envelope.Difference($changed) -ne 1){throw 'Composed frames did not identify exactly one changed pixel'}
}finally{
    if($changed){$changed.Dispose()}
    $baseline.Dispose()
}
$images=@{};$tint=[Drawing.Color]::FromArgb(143,255,255,255)
try{
    foreach($name in 'held','final','double','gap','darker','stale','hole'){$images[$name]=[Drawing.Bitmap]::new(60,24)}
    for($y=0;$y -lt 24;$y++){for($x=0;$x -lt 60;$x++){
        $strip=$x -ge 40 -and $x -lt 52;$icon=$strip -and $x -ge 44 -and $x -lt 47 -and $y -ge 10 -and $y -lt 13
        $held=if($x -ge 20 -and $x -lt 23 -and $y -ge 5 -and $y -lt 8){46}elseif($x -ge 10 -and $x -lt 56){224}else{184}
        $final=if($icon){46}elseif($strip){224}elseif($x -ge 37 -and $x -lt 40){178}else{184}
        $values=@{held=$held;final=$final;double=$(if($strip -and !$icon){241}else{$final});gap=$(if($strip -and !$icon){184}else{$final});darker=$(if($strip -and !$icon){200}else{$final})}
        $values.stale=if($x -ge 20 -and $x -lt 30 -and $y -ge 15 -and $y -lt 17){100}else{$values.double}
        $values.hole=if($x -ge 48 -and $x -lt 51 -and $y -ge 16 -and $y -lt 20){184}else{$values.double}
        foreach($name in $values.Keys){$images[$name].SetPixel($x,$y,[Drawing.Color]::FromArgb($values[$name],$values[$name],$values[$name]))}
    }}
    $envelope=[TabFrames+Envelope]::new($images.held,$images.final);$column=[Drawing.Rectangle]::new(40,0,12,24)
    foreach($name in 'double','gap','darker','stale','hole'){if($envelope.Difference($images[$name]) -le 8){throw "Composed frames did not flag the $name column state"}}
    if(!$envelope.ExtraTint($images.double,$images.final,$column,[Drawing.Rectangle]::Empty,$tint)){throw 'Composed frames did not recognize one extra column tint'}
    foreach($name in 'gap','darker','stale','hole'){if($envelope.ExtraTint($images[$name],$images.final,$column,[Drawing.Rectangle]::Empty,$tint)){throw "Composed frames accepted the $name column state as one extra tint"}}
}finally{foreach($image in $images.Values){$image.Dispose()}}
Write-Output 'Composed frames actual helper compiled and compared memory pixels'
