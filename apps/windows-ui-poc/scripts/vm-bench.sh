#!/usr/bin/env bash
set -euo pipefail
script_directory=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
python3 - "$script_directory" "$@" <<'PY'
import argparse
import base64
import hashlib
import io
import json
from pathlib import Path
import re
import subprocess
import tarfile
import time
import urllib.request

parser = argparse.ArgumentParser()
parser.add_argument('scripts', type=Path)
parser.add_argument('--vm', required=True, choices=('capycanvas6-poc-gpu', 'capycanvas6-poc-warp'))
parser.add_argument('--profile', choices=('normal', 'lowcpu'), default='normal')
parser.add_argument('--already-started', action='store_true')
args = parser.parse_args()
repo = args.scripts.resolve().parents[2]
vm = repo / 'tools/windows-vm/windows-vm.py'
scratch = Path('/mnt/localssd/capycanvas6-poc')
revision = subprocess.check_output(['git', '-C', str(repo), 'rev-parse', 'HEAD'], text=True).strip()
run_name = time.strftime('%Y%m%d-%H%M%S') + '-' + revision[:9] + '-' + args.profile
local = scratch / 'results' / args.vm / run_name
local.parent.mkdir(parents=True, exist_ok=True)
tools = scratch / 'tools'
tools.mkdir(parents=True, exist_ok=True)
filename = 'PresentMon-2.3.0-x64.exe'
url = 'https://github.com/GameTechDev/PresentMon/releases/download/v2.3.0/' + filename
expected_sha256 = '690533cf5c2591ef571a3037aa4c205c8e75860ef3f56927bf3a98acf7198485'
presentmon = tools / filename
if not presentmon.exists():
    with urllib.request.urlopen(url, timeout=120) as response, presentmon.open('wb') as output:
        while chunk := response.read(1024 * 1024):
            output.write(chunk)
sha256 = hashlib.sha256(presentmon.read_bytes()).hexdigest()
if sha256 != expected_sha256:
    raise SystemExit('Pinned PresentMon SHA256 mismatch')
metadata = dict(url=url, sha256=sha256, bytes=presentmon.stat().st_size, version='2.3.0')
(tools / (filename + '.json')).write_text(json.dumps(metadata, indent=2) + '\n')
def call(*command, **kwargs):
    return subprocess.run(['python3', str(vm), '--vm', args.vm, *command], check=True, **kwargs)
def ssh(script, **kwargs):
    return call('ssh', script, **kwargs)
started = args.already_started
failure = None
try:
    if not args.already_started:
        available = int(re.search(r'^MemAvailable:\s+(\d+)', Path('/proc/meminfo').read_text(), re.M)[1])
        if available < 56 * 1024 * 1024:
            raise RuntimeError(f'MemAvailable below 56 GiB: {available} KiB')
        call('start', *(['--gpu'] if args.vm.endswith('-gpu') else []))
        started = True
    call('sync')
    guest_root = r'C:\capycanvas\artifacts\windows-ui-poc'
    guest_tools = guest_root + r'\tools'
    guest_out = guest_root + '\\' + run_name
    ssh(f"New-Item -ItemType Directory -Force '{guest_tools}'|Out-Null")
    payload = io.BytesIO()
    with tarfile.open(fileobj=payload, mode='w') as archive:
        for source in (presentmon, tools / (filename + '.json')):
            archive.add(source, arcname=source.name)
    ssh(f"tar -xmf - -C {guest_tools}", input=payload.getvalue())
    ssh(f"Set-Content -LiteralPath 'C:\\capycanvas\\apps\\windows-ui-poc\\source-revision.txt' -Value '{revision}' -Encoding ASCII; & 'C:\\Program Files\\PowerShell\\7\\pwsh.exe' -NoProfile -File 'C:\\capycanvas\\apps\\windows-ui-poc\\scripts\\build.ps1'; exit $LASTEXITCODE")
    task = 'CapyPoc-' + run_name
    runner_path = guest_out + r'\invoke.ps1'
    invocation = f"& 'C:\\capycanvas\\apps\\windows-ui-poc\\scripts\\run-bench.ps1' -Vm {args.vm} -Profile {args.profile} -Out '{guest_out}' -PresentMon '{guest_tools}\\{filename}' -PresentMonSha256 {sha256}"
    runner = f"""$ErrorActionPreference='Stop'
$exitCode=1
Start-Transcript -LiteralPath '{guest_out}\\task.log'|Out-Null
try{{{invocation};$exitCode=0}}catch{{$_|Out-String|Set-Content -LiteralPath '{guest_out}\\task-failure.txt';Write-Error $_ -ErrorAction Continue}}finally{{@{{exit_code=$exitCode}}|ConvertTo-Json|Set-Content -LiteralPath '{guest_out}\\task-result.json';Stop-Transcript|Out-Null}}
exit $exitCode
"""
    encoded = base64.b64encode(runner.encode('utf-8')).decode('ascii')
    ssh(f"New-Item -ItemType Directory -Force '{guest_out}'|Out-Null;[IO.File]::WriteAllBytes('{runner_path}',[Convert]::FromBase64String('{encoded}'))")
    arguments = f'''"C:\\Program Files\\PowerShell\\7\\pwsh.exe" -NoProfile -Sta -ExecutionPolicy Bypass -File "{runner_path}"'''
    ssh(f"""$ErrorActionPreference='Stop'
$deadline=(Get-Date).AddMinutes(2)
while(!(Get-Process explorer -IncludeUserName -ErrorAction SilentlyContinue|Where-Object UserName -Like '*\\capy')){{if((Get-Date)-ge$deadline){{throw 'Interactive desktop unavailable'}};Start-Sleep 2}}
$action=New-ScheduledTaskAction -Execute conhost.exe -Argument '{arguments}' -WorkingDirectory 'C:\\capycanvas'
$principal=New-ScheduledTaskPrincipal -UserId capy -LogonType Interactive -RunLevel Limited
$settings=New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit (New-TimeSpan -Hours 12)
Register-ScheduledTask -TaskName '{task}' -Action $action -Principal $principal -Settings $settings|Out-Null
Start-ScheduledTask -TaskName '{task}'
""")
    deadline = time.monotonic() + 43200
    while True:
        status = ssh(f"$task=Get-ScheduledTask -TaskName '{task}';$info=Get-ScheduledTaskInfo -TaskName '{task}';@{{state=[string]$task.State;exit_code=$info.LastTaskResult;done=(Test-Path -LiteralPath '{guest_out}\\done.json')}}|ConvertTo-Json -Compress", capture_output=True, text=True)
        state = json.loads(status.stdout)
        if state['state'] not in ('Running', 'Queued'):
            break
        if time.monotonic() > deadline:
            raise RuntimeError(f'Owned benchmark task exceeded deadline: {task}')
        time.sleep(3)
    archive_path = local.parent / (run_name + '.tar')
    with archive_path.open('wb') as stream:
        ssh(f'tar -cf - -C {guest_root} {run_name}', stdout=stream)
    with tarfile.open(archive_path) as archive:
        for member in archive.getmembers():
            if not (member.name == run_name or member.name.startswith(run_name + '/')):
                raise RuntimeError('Guest archive escaped the requested run')
        archive.extractall(local.parent, filter='data')
    archive_path.unlink()
    ssh(f"Unregister-ScheduledTask -TaskName '{task}' -Confirm:$false")
    if not (local / 'done.json').exists():
        raise RuntimeError(f'Benchmark task ended without done.json: {state}; retrieved evidence: {local}')
    done = json.loads((local / 'done.json').read_text(encoding='utf-8-sig'))
    if not done['ok'] or state['exit_code'] != 0:
        raise RuntimeError(f'Benchmark gate failed; raw evidence: {local}')
except BaseException as error:
    failure = error
finally:
    if started:
        call('stop')
if failure:
    raise failure
subprocess.run(['python3', str(args.scripts / 'report.py'), str(local), '--out', str(repo / 'artifacts/windows-ui-poc')], check=True)
print(f'Raw evidence: {local}', flush=True)
PY
