$ErrorActionPreference='Stop'
$scripts=Join-Path $PSScriptRoot '../scripts'
function Read-Ast([string]$Name){
    $tokens=$null;$errors=$null
    $ast=[Management.Automation.Language.Parser]::ParseFile((Join-Path $scripts $Name),[ref]$tokens,[ref]$errors)
    if(@($errors).Count){throw "$Name failed parsing: $($errors.Message -join '; ')"};$ast
}
function Import-NativeType($Ast,[string]$Name){
    if($Name -as [type]){return}
    $type=@($Ast.FindAll({param($n)$n -is [Management.Automation.Language.StringConstantExpressionAst] -and $n.Value.Contains('public static class '+$Name)}.GetNewClosure(),$true))
    if($type.Count -ne 1){throw "Missing actual native API declaration: $Name"};Add-Type -TypeDefinition $type[0].Value
}
$inspect=Read-Ast 'inspect-window.ps1';$localization=Read-Ast 'exercise-localization.ps1'
Import-NativeType $inspect 'CapyWindowCapture';Import-NativeType (Read-Ast 'CapyUia.ps1') 'CapyWindowApi'
Add-Type -TypeDefinition @'
using System;using System.Runtime.InteropServices;
public static class CapyCaptureDpiTest {
    [DllImport("user32.dll")]static extern IntPtr GetThreadDpiAwarenessContext();
    [DllImport("user32.dll")]static extern bool AreDpiAwarenessContextsEqual(IntPtr a,IntPtr b);
    public static bool Is(int context){return AreDpiAwarenessContextsEqual(GetThreadDpiAwarenessContext(),new IntPtr(context));}
}
'@
$scopes=@(@{name='inspect-window';body=$inspect.EndBlock;variable='$captureDpi';api='CapyWindowCapture';boundaries=@('$p=Get-Process','$graphics.CopyFromScreen')})
foreach($name in @('Canvas-Pixels','Stable-CanvasPixels')){
    $fn=@($localization.FindAll({param($n)$n -is [Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq $name}.GetNewClosure(),$true))
    if($fn.Count -ne 1){throw "Missing actual capture function: $name"}
    $scopes+=@{name=$name;body=$fn[0].Body.EndBlock;variable=$(if($name -eq 'Canvas-Pixels'){'$dpi'}else{'$phaseDpi'});api='CapyWindowApi';boundaries=$(if($name -eq 'Canvas-Pixels'){@('Prepare-CanvasPixels','$graphics.CopyFromScreen')}else{@('Prepare-CanvasPixels','Wait-StablePixels','$after=Canvas-Projection')})}
}
$original=[CapyWindowApi]::SetThreadDpiAwarenessContext([IntPtr](-1))
try{
    foreach($scope in $scopes){
        $statements=@($scope.body.Statements)
        $entry=@($statements|Where-Object {$_ -is [Management.Automation.Language.AssignmentStatementAst] -and $_.Left.Extent.Text -eq $scope.variable})
        $guard=@($statements|Where-Object {$_ -is [Management.Automation.Language.TryStatementAst]})
        $setter="[$($scope.api)]::SetThreadDpiAwarenessContext([IntPtr](-4))"
        $restorer="[$($scope.api)]::SetThreadDpiAwarenessContext($($scope.variable))"
        if($entry.Count -ne 1 -or $guard.Count -ne 1 -or !$guard[0].Finally -or !$entry[0].Right.Extent.Text.Contains($setter) -or !$guard[0].Finally.Extent.Text.Contains($restorer) -or $entry[0].Extent.StartOffset -gt $guard[0].Extent.StartOffset){throw "$($scope.name) lacks a balanced actual physical DPI scope"}
        foreach($boundary in $scope.boundaries){if(!$guard[0].Body.Extent.Text.Contains($boundary)){throw "$($scope.name) prepares or captures outside its physical DPI scope"}}
        $probe=[scriptblock]::Create($entry[0].Extent.Text+"`ntry {if(![CapyCaptureDpiTest]::Is(-4)){throw 'Capture body lacks physical DPI'};if(`$shouldThrow){throw 'Controlled capture exception'}}finally "+$guard[0].Finally.Extent.Text)
        foreach($shouldThrow in @($false,$true)){
            $caught=$false
            try{. $probe}catch{if($_.Exception.Message -cne 'Controlled capture exception'){throw};$caught=$true}
            if($caught -ne $shouldThrow -or ![CapyCaptureDpiTest]::Is(-1)){throw "$($scope.name) leaked caller DPI on success or exception"}
        }
    }
}finally{[CapyWindowApi]::SetThreadDpiAwarenessContext($original)|Out-Null}
Write-Output 'Capture DPI scopes passed: actual inspect and localization setter/finally AST, physical preparation boundaries, success and exception restoration; no UI or GPU.'

Add-Type -AssemblyName WindowsBase
$clearAst=Read-Ast 'CapyUia.ps1'
foreach($name in @('Find-ClearCanvasSamplePoint','Clear-CanvasPoint')){
    $fn=@($clearAst.FindAll({param($n)$n-is[Management.Automation.Language.FunctionDefinitionAst]-and$n.Name-eq$name}.GetNewClosure(),$true))
    if($fn.Count-ne1){throw "Missing actual sampling function: $name"};Invoke-Expression $fn[0].Extent.Text
}
Add-Type -TypeDefinition @'
using System;
public static class CapyClearSampleOracle {
 public static int[] Find(double[] canvas,double[] paper,double[] panel,int dx,int dy) {
  int loX=(int)Math.Floor(canvas[0])-1,hiX=(int)Math.Ceiling(canvas[0]+canvas[2])+1;
  int loY=(int)Math.Floor(canvas[1])-1,hiY=(int)Math.Ceiling(canvas[1]+canvas[3])+1;
  if((long)(hiX-loX+1)*(hiY-loY+1)>25000)throw new Exception("Brute-force case exceeds its independent bounded domain");
  for(int y=loY;y<=hiY;y++)for(int x=loX;x<=hiX;x++) {
   double left=x-48+Math.Min(0,dx),top=y-48+Math.Min(0,dy),right=left+96+Math.Abs((double)dx),bottom=top+96+Math.Abs((double)dy);
   bool inCanvas=left>=canvas[0]&&top>=canvas[1]&&right<=canvas[0]+canvas[2]&&bottom<=canvas[1]+canvas[3];
   bool inPaper=left>=paper[0]&&top>=paper[1]&&right<=paper[0]+paper[2]&&bottom<=paper[1]+paper[3];
   bool touchesUtility=left<=panel[0]+panel[2]&&right>=panel[0]&&top<=panel[1]+panel[3]&&bottom>=panel[1];
   if(inCanvas&&inPaper&&!touchesUtility)return new int[]{x,y};
  }
  return null;
 }
}
'@
$sampleRecords=[Collections.Generic.List[object]]::new()
function Rect([double[]]$Values){[Windows.Rect]::new($Values[0],$Values[1],$Values[2],$Values[3])}
function Vector([Windows.Rect]$Value){@($Value.X,$Value.Y,$Value.Width,$Value.Height)}
function Sample([int[]]$Point,[int]$Dx,[int]$Dy){[Windows.Rect]::new($Point[0]-48+[Math]::Min(0,$Dx),$Point[1]-48+[Math]::Min(0,$Dy),96+[Math]::Abs([double]$Dx),96+[Math]::Abs([double]$Dy))}
function Check-Sample([string]$Name,[Windows.Rect]$Canvas,[Windows.Rect]$Paper,[Windows.Rect]$Panel,[int]$Dx=0,[int]$Dy=0,[switch]$SkipOracle){
    $expected=if(!$SkipOracle){[CapyClearSampleOracle]::Find((Vector $Canvas),(Vector $Paper),(Vector $Panel),$Dx,$Dy)}else{$null}
    $point=$null;$errorText=''
    try{$point=@(Find-ClearCanvasSamplePoint $Canvas $Paper $Panel $Dx $Dy)}catch{$errorText=$_.Exception.Message;if($errorText-cne'No clear visible paper sample fits outside the utility'){throw}}
    if(!$SkipOracle-and($null-ne$expected)-ne($null-ne$point)){throw "$Name disagrees with independent brute force: expected=$($null-ne$expected), actual=$($null-ne$point), error=$errorText"}
    if($point){
        if($point.Count-ne2-or$point[0]-isnot[int]-or$point[1]-isnot[int]){throw "$Name did not return an integer centre"}
        $footprint=Sample $point $Dx $Dy
        if(!$Canvas.Contains($footprint)-or!$Paper.Contains($footprint)-or$Panel.IntersectsWith($footprint)){throw "$Name violated an original exact sampling predicate"}
    }
    $sampleRecords.Add([ordered]@{name=$Name;sample_exists=($null-ne$point);oracle_evaluated=(!$SkipOracle);oracle_exists=$(if($SkipOracle){$null}else{$null-ne$expected});point=$point;Dx=$Dx;Dy=$Dy;rejection=$errorText})
    ,$point
}
function Reject-Geometry([string]$Name,[Windows.Rect]$Canvas,[Windows.Rect]$Paper,[Windows.Rect]$Panel,[string]$Message){
    $caught='';try{$null=Find-ClearCanvasSamplePoint $Canvas $Paper $Panel}catch{$caught=$_.Exception.Message}
    if(!$caught.Contains($Message)){throw "$Name failed to reject invalid geometry: $caught"}
    $sampleRecords.Add([ordered]@{name=$Name;rejected=$true;message=$caught})
}
$baseRect=Rect @(0,0,96,96)
Check-Sample 'exact96-fit' $baseRect $baseRect (Rect @(200,200,1,1))|Out-Null
Check-Sample 'left-edge-touch-is-intersection' $baseRect $baseRect (Rect @(96,0,1,96))|Out-Null
Check-Sample 'fractional-left-gap-keeps-integer48' $baseRect $baseRect (Rect @(96.01,0,1,96))|Out-Null
Check-Sample 'fractional-left-overlap-refused' $baseRect $baseRect (Rect @(95.99,0,1,96))|Out-Null
Check-Sample 'right-edge-touch-is-intersection' $baseRect $baseRect (Rect @(-1,0,1,96))|Out-Null
Check-Sample 'fractional-right-gap-keeps-integer48' $baseRect $baseRect (Rect @(-1.01,0,1,96))|Out-Null
Check-Sample 'top-edge-touch-is-intersection' $baseRect $baseRect (Rect @(0,96,96,1))|Out-Null
Check-Sample 'fractional-top-gap-keeps-integer48' $baseRect $baseRect (Rect @(0,96.01,96,1))|Out-Null
Check-Sample 'bottom-edge-touch-is-intersection' $baseRect $baseRect (Rect @(0,-1,96,1))|Out-Null
Check-Sample 'fractional-bottom-gap-keeps-integer48' $baseRect $baseRect (Rect @(0,-1.01,96,1))|Out-Null
Check-Sample 'full-occlusion' (Rect @(0,0,100,100)) (Rect @(0,0,100,100)) (Rect @(0,0,100,100))|Out-Null
Check-Sample 'paper-clipped-by-canvas-no-fit' (Rect @(0,0,95,100)) (Rect @(0,0,100,100)) (Rect @(200,200,1,1))|Out-Null
Check-Sample 'paper-canvas-disjoint' $baseRect (Rect @(200,200,96,96)) (Rect @(400,400,1,1))|Out-Null
Check-Sample 'positive-stroke-footprint' (Rect @(0,0,104,108)) (Rect @(0,0,104,108)) (Rect @(200,200,1,1)) 8 12|Out-Null
Check-Sample 'negative-stroke-footprint' (Rect @(0,0,104,108)) (Rect @(0,0,104,108)) (Rect @(200,200,1,1)) -8 -12|Out-Null
Check-Sample 'mixed-stroke-footprint' (Rect @(-5.5,-4.25,111,110)) (Rect @(-5.5,-4.25,111,110)) (Rect @(200,200,1,1)) -9 7|Out-Null
Check-Sample 'pressure40x8-footprint' (Rect @(0,0,136,104)) (Rect @(0,0,136,104)) (Rect @(200,200,1,1)) 40 8|Out-Null
Check-Sample 'negative-pressure40x8-footprint' (Rect @(0,0,136,104)) (Rect @(0,0,136,104)) (Rect @(200,200,1,1)) -40 -8|Out-Null
Check-Sample 'one-pixel-too-wide-footprint' $baseRect $baseRect (Rect @(200,200,1,1)) 1 0|Out-Null
Reject-Geometry 'empty-canvas' ([Windows.Rect]::Empty) $baseRect $baseRect 'nonempty'
Reject-Geometry 'zero-paper' $baseRect (Rect @(0,0,0,96)) $baseRect 'nonempty'
Reject-Geometry 'zero-utility' $baseRect $baseRect (Rect @(0,0,96,0)) 'nonempty'
Reject-Geometry 'NaN-origin' (Rect @([double]::NaN,0,96,96)) $baseRect $baseRect 'finite'
Reject-Geometry 'infinite-width' (Rect @(0,0,[double]::PositiveInfinity,96)) $baseRect $baseRect 'finite'
Reject-Geometry 'overflowing-edge' (Rect @(1e308,0,1e308,96)) $baseRect $baseRect 'finite'
for($i=0;$i-lt2048;$i++){
    $x=@(-1.75,-.5,0,.25,1.5)[$i%5];$y=@(-1.25,0,.5,1.75)[[int][Math]::Floor($i/5)%4]
    $width=96+$i%9;$height=96+[int][Math]::Floor($i/9)%9
    $canvas=Rect @($x,$y,$width,$height)
    $paper=Rect @(($x+@(-2,-.25,0,.5,2)[[int][Math]::Floor($i/81)%5]),($y+@(-2,-.5,0,.25,2)[[int][Math]::Floor($i/405)%5]),(96+[int][Math]::Floor($i/3)%11),(96+[int][Math]::Floor($i/7)%11))
    $panel=Rect @(($x+@(-4,-.01,0,2,48,94,96.01,104)[$i%8]),($y+@(-4,-.01,0,2,48,94,96.01,104)[[int][Math]::Floor($i/8)%8]),(@(.25,2,48,96,110)[[int][Math]::Floor($i/64)%5]),(@(.25,2,48,96,110)[[int][Math]::Floor($i/320)%5]))
    $dx=@(-7,-1,0,1,7)[[int][Math]::Floor($i/11)%5];$dy=@(-7,-1,0,1,7)[[int][Math]::Floor($i/13)%5]
    Check-Sample ("brute-$i") $canvas $paper $panel $dx $dy|Out-Null
}
$script:recordedModel=[pscustomobject]@{state=[pscustomobject]@{
    camera=[pscustomobject]@{viewport=@(2880,1824);zoom=0.59624999761581421;rotation=0;flipped=@($false,$false);translation=@(676.79998779296875,423.5999755859375);work_area=@(592,180,1696,1632)}
    tabs=@([pscustomobject]@{active=$true;width=2560;height=1920})
}}
$script:recordedBounds=Rect @(0,0,2880,1824)
$script:recordedPanel=Rect @(1088,252,768,1284)
function Control([string]$Id,[switch]$Arranged){[pscustomobject]@{Current=[pscustomobject]@{BoundingRectangle=$(if($Id-eq'drawing-canvas'){$script:recordedBounds}else{$script:recordedPanel})}}}
function Model{$script:recordedModel}
$oldFailure=''
if($env:CAPY_CLEAR_GEOMETRY_OLD){
    $oldTokens=$null;$oldErrors=$null;$oldAst=[Management.Automation.Language.Parser]::ParseFile($env:CAPY_CLEAR_GEOMETRY_OLD,[ref]$oldTokens,[ref]$oldErrors)
    if($oldErrors.Count){throw 'Recorded grid source failed parsing'}
    $oldFn=@($oldAst.FindAll({param($n)$n-is[Management.Automation.Language.FunctionDefinitionAst]-and$n.Name-eq'Clear-CanvasPoint'},$true))[0]
    Invoke-Expression ($oldFn.Extent.Text.Replace('function Clear-CanvasPoint','function Old-ClearCanvasPoint'))
    try{$null=Old-ClearCanvasPoint 'pen-tilt-dialog'}catch{$oldFailure=$_.Exception.Message}
    if($oldFailure-cne'No clear visible paper sample fits outside the utility'){throw 'Old actual grid did not reproduce its recorded false negative'}
}
$point=@(Clear-CanvasPoint 'pen-tilt-dialog')
$camera=$recordedModel.state.camera;$tab=@($recordedModel.state.tabs|Where-Object active)[0]
$left=[Math]::Max($camera.work_area[0],$camera.translation[0]);$right=[Math]::Min($camera.work_area[0]+$camera.work_area[2],$camera.translation[0]+$tab.width*$camera.zoom)
$top=[Math]::Max($camera.work_area[1],$camera.translation[1]);$bottom=[Math]::Min($camera.work_area[1]+$camera.work_area[3],$camera.translation[1]+$tab.height*$camera.zoom)
$paper=Rect @(($recordedBounds.X+$left),($recordedBounds.Y+$top),($right-$left),($bottom-$top))
$null=Check-Sample 'actual-recorded-fitted-failure' $recordedBounds $paper $recordedPanel -SkipOracle
$footprint=Sample $point 0 0
if(!$recordedBounds.Contains($footprint)-or!$paper.Contains($footprint)-or$recordedPanel.IntersectsWith($footprint)){throw 'Actual wrapper did not return the exact clear96px sample'}
$sampleRecords.Add([ordered]@{name='actual-recorded-wrapper-positive';old_grid_negative_included=[bool]$env:CAPY_CLEAR_GEOMETRY_OLD;old_failure=$oldFailure;point=$point;footprint=(Vector $footprint);paper=(Vector $paper);canvas=(Vector $recordedBounds);utility=(Vector $recordedPanel)})
$result=[ordered]@{runtime=$PSVersionTable.PSVersion.ToString();helper_sha256=(Get-FileHash -LiteralPath (Join-Path $scripts 'CapyUia.ps1')).Hash;test_sha256=(Get-FileHash -LiteralPath $PSCommandPath).Hash;case_count=$sampleRecords.Count;bounded_bruteforce_cases=2048;records=$sampleRecords;actual_AST_import=$true;independent_oracle='Enumerates integer centres and original containment/intersection predicates; no feasible-range formulas';recorded_case='8a2-tilt-dark-fitted-evidence-full camera/native utility geometry';source_uses_no_live_UIA_provider_app_GPU=$true}
if($env:CAPY_CLEAR_GEOMETRY_OUTPUT){[IO.File]::WriteAllText($env:CAPY_CLEAR_GEOMETRY_OUTPUT,($result|ConvertTo-Json -Depth 12),[Text.UTF8Encoding]::new($false))}
Write-Output "Clear canvas geometry passed $($sampleRecords.Count) cases, including2048 independent bounded brute-force comparisons; recorded old-grid negative included=$([bool]$env:CAPY_CLEAR_GEOMETRY_OLD); no UI/GPU."

Add-Type -TypeDefinition @'
public static class CapyUtilityParkingTest {
 public static int X,Y;
 public static void Hover(int x,int y){X=x;Y=y;}
}
'@
& {
    $tiltAst=Read-Ast 'exercise-tilt.ps1'
    foreach($name in @('Canvas-Point','Park-UtilityCapture')){
        $fn=@($tiltAst.FindAll({param($n)$n-is[Management.Automation.Language.FunctionDefinitionAst]-and$n.Name-eq$name}.GetNewClosure(),$true))
        if($fn.Count-ne1){throw "Missing actual tilt parking function: $name"}
        . ([scriptblock]::Create($fn[0].Extent.Text.Replace('[CapyRowPointer]','[CapyUtilityParkingTest]')))
    }
    $review=[pscustomobject]@{Id=7};$scale=1.5
    $root=[pscustomobject]@{Current=[pscustomobject]@{BoundingRectangle=$recordedBounds}}
    $forceTooltip=$false
    function Capy-Tooltips([int]$ProcessId){
        if($ProcessId-ne7){throw 'Tooltip parking queried another process'}
        if($forceTooltip-or!$paper.Contains([double][CapyUtilityParkingTest]::X,[double][CapyUtilityParkingTest]::Y)-or$recordedPanel.Contains([double][CapyUtilityParkingTest]::X,[double][CapyUtilityParkingTest]::Y)){'Document Title'}
    }
    function Wait-Until([scriptblock]$Condition,[string]$Message){if(!(& $Condition)){throw $Message}}
    Park-UtilityCapture
    if([CapyUtilityParkingTest]::X-ne$point[0]-or[CapyUtilityParkingTest]::Y-ne$point[1]){throw 'Tilt capture did not park on its model-derived clear paper point'}
    $forceTooltip=$true;$caught=''
    try{Park-UtilityCapture}catch{$caught=$_.Exception.Message}
    if($caught-cne'Utility tooltip did not leave the capture area'){throw 'Tilt capture lost its unchanged zero-tooltip assertion'}
}
Write-Output 'Tilt utility capture parks on actual model-derived clear paper, outside the utility and header, and still rejects any tooltip.'
