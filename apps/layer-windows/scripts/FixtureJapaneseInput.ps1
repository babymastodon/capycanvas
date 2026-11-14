$FixtureJapaneseTip='0411:{03B5835F-F03C-411B-9CE2-AA23E1171E36}{A76C93D9-5523-4E90-AAFA-4DB112F9AC76}'
function Add-FixtureJapaneseInput{
    $languages=Get-WinUserLanguageList
    if($languages|Where-Object {$_.InputMethodTips -contains $FixtureJapaneseTip}){return $null}
    $japanese=@($languages|Where-Object {$_.LanguageTag -in 'ja','ja-JP'})[0]
    $created=!$japanese
    if($created){$japanese=(New-WinUserLanguageList -Language 'ja-JP')[0];$languages.Add($japanese)}
    if($japanese.InputMethodTips -notcontains $FixtureJapaneseTip){$japanese.InputMethodTips.Add($FixtureJapaneseTip)}
    Set-WinUserLanguageList -LanguageList $languages -Force
    $created
}
function Remove-FixtureJapaneseInput([switch]$Language){
    $languages=Get-WinUserLanguageList
    $owners=@($languages|Where-Object {$_.InputMethodTips -contains $FixtureJapaneseTip})
    if(!$owners.Count){return}
    foreach($owner in $owners){
        $null=$owner.InputMethodTips.Remove($FixtureJapaneseTip)
        if($Language -or !$owner.InputMethodTips.Count){$null=$languages.Remove($owner)}
    }
    Set-WinUserLanguageList -LanguageList $languages -Force
}
