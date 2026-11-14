param([string]$Source=(Join-Path $PSScriptRoot '../scripts'))
$ErrorActionPreference='Stop'
. (Join-Path $Source 'NavigatorFrames.ps1')
$run=Join-Path $PSScriptRoot ('../../../artifacts/windows/tests/navigator-'+[guid]::NewGuid().ToString('N'))
[IO.Directory]::CreateDirectory($run)|Out-Null
try{Navigator-LoadTypes;Navigator-TestClassifier;'Navigator geometry and pixel oracle: passed'}finally{Remove-Item -LiteralPath $run -Recurse -Force}
