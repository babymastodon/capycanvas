import base64, pathlib, sys
key, url, output = sys.argv[1:]
pathlib.Path(output).write_text('\n'.join([
    '[Flatpak Ref]', 'Name=art.capycanvas.CapyCanvas', 'Branch=stable',
    'Title=Capy Canvas', 'IsRuntime=false', 'SuggestRemoteName=capycanvas',
    'Url=' + url, 'RuntimeRepo=https://dl.flathub.org/repo/flathub.flatpakrepo',
    'GPGKey=' + base64.b64encode(pathlib.Path(key).read_bytes()).decode(), '',
]))
