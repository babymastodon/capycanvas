param(
    [Parameter(Mandatory)][string]$Executable,
    [Parameter(Mandatory)][string]$Output,
    [Parameter(Mandatory)][string]$BuildRecord,
    [Parameter(Mandatory)][string]$QueueConfig,
    [ValidateSet('release','dev-perf')][string]$RustProfile='release',
    [ValidateRange(1,1440)][int]$TimeoutMinutes=20
)
$ErrorActionPreference='Stop'
function Set-UserValue([string]$Path,[string]$Name,[int]$Value){
    if(!(Test-Path -LiteralPath $Path)){New-Item -Path $Path -Force|Out-Null}
    New-ItemProperty -Path $Path -Name $Name -Value $Value -PropertyType DWord -Force|Out-Null
}
try{
    New-Item -ItemType Directory -Force $Output,([Environment]::GetFolderPath([Environment+SpecialFolder]::DesktopDirectory))|Out-Null
    $personalize='HKCU:\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize'
    Set-UserValue $personalize AppsUseLightTheme 0
    Set-UserValue $personalize SystemUsesLightTheme 0
    & (Join-Path $PSScriptRoot 'run-fixtures.ps1') -Executable $Executable -Output $Output -BuildRecord $BuildRecord -QueueConfig $QueueConfig -RustProfile $RustProfile -TimeoutMinutes $TimeoutMinutes -HardwareGpu -SharedSession
}catch{
    ($_|Out-String)+$_.ScriptStackTrace|Set-Content (Join-Path $Output 'runner-error.txt')
    exit 1
}
