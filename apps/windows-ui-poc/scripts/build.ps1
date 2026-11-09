param([string]$TargetDirectory)
$ErrorActionPreference='Stop'
$root=(Resolve-Path (Join-Path $PSScriptRoot '..')).Path
if(!$TargetDirectory){$TargetDirectory=Join-Path $root 'target'}
$env:CARGO_TARGET_DIR=$TargetDirectory
$env:CARGO_TERM_COLOR='never'
$target=Join-Path $TargetDirectory 'release'
Push-Location $root
try{
    & cargo build --release --locked --workspace --bins --lib
    if($LASTEXITCODE){throw "Rust release build exited $LASTEXITCODE"}
    $project=Join-Path $root 'winui/PocWinUI.vcxproj'
    if(Test-Path -LiteralPath $project){
        $vswhere=Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio/Installer/vswhere.exe'
        $msbuild=& $vswhere -latest -products '*' -requires Microsoft.Component.MSBuild -find 'MSBuild/**/Bin/MSBuild.exe'|Select-Object -First 1
        if(!$msbuild){throw 'MSBuild was not found'}
        $nuget=Get-Command nuget.exe -ErrorAction SilentlyContinue
        if($nuget){$nugetPath=$nuget.Source}else{$nugetPath=(Get-Item -LiteralPath (Join-Path $env:USERPROFILE '.local/tools/nuget/nuget.exe')).FullName}
        & $nugetPath restore (Join-Path $root 'winui/packages.config') -PackagesDirectory (Join-Path $root 'winui/packages') -NonInteractive
        if($LASTEXITCODE){throw 'WinUI NuGet restore failed'}
        & $msbuild $project /m /p:Configuration=Release /p:Platform=x64 ("/p:OutDir=$target\") ("/p:IntDir=$TargetDirectory\winui-intermediate\")
        if($LASTEXITCODE){throw 'WinUI release build failed'}
    }
    $files=@(Get-ChildItem -LiteralPath $target -File -Recurse|Where-Object {$_.Extension -in '.exe','.dll'}|Sort-Object FullName|ForEach-Object {@{path=$_.FullName.Substring($target.Length+1);sha256=(Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant();bytes=$_.Length}})
    if(!$files.Count){throw 'Release build produced no executables or DLLs'}
    $revisionFile=Join-Path $root 'source-revision.txt'
    $revision=if(Test-Path -LiteralPath $revisionFile){(Get-Content -LiteralPath $revisionFile -Raw).Trim()}else{& git rev-parse HEAD}
    $inputs=@(Get-ChildItem -LiteralPath $root -Recurse -File|Where-Object {$_.FullName -notlike ($TargetDirectory.TrimEnd('\')+'\*') -and $_.Extension -in '.rs','.wgsl','.toml','.lock','.ps1','.cpp','.h','.vcxproj','.config','.manifest','.svg'}|Sort-Object FullName|ForEach-Object {@{path=$_.FullName.Substring($root.Length+1);sha256=(Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant()}})
    @{configuration='Release';commit=$revision;rustc=(& rustc --version);source_inputs=$inputs;files=$files}|ConvertTo-Json -Depth 8|Set-Content -LiteralPath (Join-Path $target 'build.json') -Encoding UTF8
}finally{Pop-Location}
