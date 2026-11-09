pub fn plain(path:&str)->String {
    if let Some(unc)=path.strip_prefix(r"\\?\UNC\") {return format!(r"\\{unc}");}
    if let Some(drive)=path.strip_prefix(r"\\?\") && drive.as_bytes().get(1..3)==Some(b":\\") {return drive.to_owned();}
    path.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn application_paths_use_plain_dos_or_unc_names() {
        for suffix in [r"日本語\PocWinUI.exe",r"results\canvas.jsonl",r"release",r"release\Assets\layer-eye-symbolic.svg"] {
            assert_eq!(plain(&format!(r"\\?\C:\{suffix}")),format!(r"C:\{suffix}"));
            assert_eq!(plain(&format!(r"\\?\UNC\server\share\{suffix}")),format!(r"\\server\share\{suffix}"));
        }
        for path in [r"C:\PocWinUI.exe",r"\\server\share\PocWinUI.exe",r"\\?\Volume{123}\PocWinUI.exe"] {assert_eq!(plain(path),path);}
    }
}
