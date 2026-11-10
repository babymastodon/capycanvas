#!/usr/bin/env bash
set -euo pipefail
script_directory=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
python3 - "$script_directory" "$@" <<'PY'
import argparse
import base64
import io
import json
from pathlib import Path
import re
import subprocess
import sys
import tarfile
import time

parser=argparse.ArgumentParser()
parser.add_argument('scripts',type=Path)
parser.add_argument('--vm',default='capycanvas6-perf-gpu',choices=('capycanvas6-perf-gpu',))
parser.add_argument('--control-build',default=r'C:\capy-bench\control-8f9a6281e')
parser.add_argument('--candidate-build')
parser.add_argument('--observer',default=r'C:\capy-bench\harness\windows-ui-motion.exe')
parser.add_argument('--workload',action='append',choices=('divider','column','resize-native','resize-programmatic','drawer','expansion','slider','wheel','typing','pen'),default=[])
parser.add_argument('--order',choices=('aa','abba','control'),default='aa')
parser.add_argument('--theme',choices=('dark','light'),default='dark')
parser.add_argument('--preflight',action='store_true')
parser.add_argument('--already-started',action='store_true')
parser.add_argument('--no-capture',action='store_true')
parser.add_argument('--contacts',type=int,choices=(1,3),default=3)
parser.add_argument('--profile',action='store_true')
parser.add_argument('--record-only',action='store_true')
args=parser.parse_args()
repo=args.scripts.resolve().parents[1]
vm=repo/'tools/windows-vm/windows-vm.py'
run_name=time.strftime('%Y%m%d-%H%M%S')+'-'+args.order+'-'+args.theme
guest_root=r'C:\capy-bench\results'
guest_out=guest_root+'\\'+run_name
local_root=Path('/mnt/localssd/capycanvas6-perf/results')
local=local_root/run_name
local_root.mkdir(parents=True,exist_ok=True)
task='CapyWinuiPerf-'+run_name
workloads=args.workload or ['divider','column','resize-native','resize-programmatic','drawer','expansion','slider','wheel','typing','pen']
if args.preflight:
    workloads=['divider']
if args.theme=='light':
    if 'pen' in args.workload:
        parser.error('The existing pen series supports dark theme only')
    workloads=[w for w in workloads if w!='pen']
if args.order=='abba' and not args.candidate_build:
    parser.error('--candidate-build required for ABBA')
if args.profile and (args.order!='control' or len(workloads)!=1 or args.preflight):
    parser.error('--profile requires --order control and one explicit workload')
if args.record_only and not args.profile:
    parser.error('--record-only requires --profile')
def quote(value):
    return "'"+str(value).replace("'","''")+"'"
def call(*command,**kwargs):
    return subprocess.run(['python3',str(vm),'--vm',args.vm,*command],check=True,**kwargs)
def ssh(script,**kwargs):
    return call('ssh',script,**kwargs)
def upload(path,text):
    data=base64.b64encode(text.encode('utf-8')).decode('ascii')
    ssh(f'[IO.File]::WriteAllBytes({quote(path)},[Convert]::FromBase64String({quote(data)}))')
started=args.already_started
guest_created=False
retrieved=False
failure=None
recording=False
profile_finished=False
profile_dir=guest_out+r'\profile'
prior_interval=10000
interval_changed=False
def retrieve():
    archive_path=local_root/(run_name+'.tar')
    with archive_path.open('wb') as output:
        ssh(f'tar -cf - -C {quote(guest_root)} {quote(run_name)}',stdout=output)
    with tarfile.open(archive_path) as archive:
        if any(not (member.name==run_name or member.name.startswith(run_name+'/')) for member in archive):
            raise RuntimeError('Guest archive escaped requested run')
        archive.extractall(local_root,filter='data')
    archive_path.unlink()
try:
    if not started:
        available=int(re.search(r'^MemAvailable:\s+(\d+)',Path('/proc/meminfo').read_text(),re.M)[1])
        if available < 56*1024*1024:
            raise RuntimeError(f'MemAvailable below 56GiB: {available}KiB')
        call('start','--gpu')
        started=True
    ssh(f'New-Item -ItemType Directory -Force {quote(guest_out)}|Out-Null')
    guest_created=True
    if args.profile:
        upload(r'C:\capy-bench\harness\capy-observer.wprp',(args.scripts/'windows-ui-motion.wprp').read_text())
        upload(r'C:\capy-bench\harness\capy-ui.wpaProfile',(args.scripts/'windows-ui-motion.wpaProfile').read_text())
    slots=[('control',args.control_build)] if args.order=='control' else [('control',args.control_build),('candidate',args.control_build if args.order=='aa' else args.candidate_build),('candidate',args.control_build if args.order=='aa' else args.candidate_build),('control',args.control_build)]
    commands=[]
    for workload in workloads:
        for index,(arm,build) in enumerate(slots):
            seed=build+'\\seed-'+args.theme
            output=guest_out+f'\\{index}-{arm}-{workload}'
            commands.append(f"& 'C:\\capycanvas\\tools\\performance\\windows-ui-motion.ps1' -Executable {quote(build+chr(92)+'CapyCanvas.exe')} -Observer {quote(args.observer)} -OutputDirectory {quote(output)} -SeedDirectory {quote(seed)} -Workload {workload} -Theme {args.theme} -Contacts {1 if args.profile else args.contacts}"+(' -Preflight' if args.preflight else '')+(' -NoCapture' if args.no_capture or args.profile else '')+(f' -ProfileDirectory {quote(profile_dir)}' if args.profile else ''))
    commands=["try{\n"+command+"\n}catch{$rowFailures.Add(($_|Out-String));Write-Error $_ -ErrorAction Continue}" for command in commands]
    runner="""$ErrorActionPreference='Stop'
$exitCode=1
$rowFailures=[Collections.Generic.List[string]]::new()
Start-Transcript -LiteralPath OUTLOG|Out-Null
try{
COMMANDS
if($rowFailures.Count){throw ($rowFailures -join "`n")}
$exitCode=0
}catch{$_|Out-String|Set-Content -LiteralPath FAILURE;Write-Error $_ -ErrorAction Continue}
finally{@{exit_code=$exitCode}|ConvertTo-Json|Set-Content -LiteralPath RESULT;Stop-Transcript|Out-Null}
exit $exitCode
""".replace('OUTLOG',quote(guest_out+r'\task.log')).replace('COMMANDS','\n'.join(commands)).replace('FAILURE',quote(guest_out+r'\failure.txt')).replace('RESULT',quote(guest_out+r'\task-result.json'))
    runner_path=guest_out+r'\invoke.ps1'
    upload(runner_path,runner)
    arguments=f'"C:\\Program Files\\PowerShell\\7\\pwsh.exe" -NoProfile -Sta -ExecutionPolicy Bypass -File "{runner_path}"'
    ssh(fr"""$ErrorActionPreference='Stop'
$action=New-ScheduledTaskAction -Execute conhost.exe -Argument {quote(arguments)} -WorkingDirectory 'C:\capycanvas'
$principal=New-ScheduledTaskPrincipal -UserId capy -LogonType Interactive -RunLevel Limited
$settings=New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit (New-TimeSpan -Hours 3)
Register-ScheduledTask -TaskName {quote(task)} -Action $action -Principal $principal -Settings $settings|Out-Null
Start-ScheduledTask -TaskName {quote(task)}
""")
    deadline=time.monotonic()+10800
    while True:
        result=ssh(f'$task=Get-ScheduledTask -TaskName {quote(task)};$info=Get-ScheduledTaskInfo -TaskName {quote(task)};@{{state=[string]$task.State;exit_code=$info.LastTaskResult;done=(Test-Path -LiteralPath {quote(guest_out+chr(92)+"task-result.json")});profile_ready=(Test-Path -LiteralPath {quote(profile_dir+chr(92)+"ready.json")});profile_finished=(Test-Path -LiteralPath {quote(profile_dir+chr(92)+"finished.json")})}}|ConvertTo-Json -Compress',capture_output=True,text=True)
        state=json.loads(result.stdout)
        if args.profile and state['profile_ready'] and not recording and not profile_finished:
            interval=ssh('wpr -profint',capture_output=True,text=True).stdout
            matched=re.search(r'=\s*(\d+)',interval)
            if not matched:
                raise RuntimeError(f'Could not preserve WPR sampling interval: {interval}')
            prior_interval=int(matched[1])
            upload(guest_out+r'\prior-interval.txt',interval)
            ssh("wpr -setprofint 1221;if($LASTEXITCODE){throw 'WPR interval failed'}")
            interval_changed=True
            try:
                ssh("wpr -start CPU -start GPU -start DesktopComposition -start XAMLActivity -start C:\\capy-bench\\harness\\capy-observer.wprp!CapyObserver -filemode -instancename CapyWinuiPerf;if($LASTEXITCODE){throw 'WPR start failed'}")
            except subprocess.CalledProcessError as error:
                upload(guest_out+r'\profile-gap.txt',f'Full XAML/composition profile failed; CPU-only fallback: {error}\n')
                ssh("wpr -cancel -instancename CapyWinuiPerf;wpr -start CPU -start C:\\capy-bench\\harness\\capy-observer.wprp!CapyObserver -filemode -instancename CapyWinuiPerf;if($LASTEXITCODE){throw 'CPU-only WPR fallback failed'}")
            recording=True
            ssh(f"[IO.File]::WriteAllText({quote(profile_dir+chr(92)+'go.json')},'{{}}')")
        if args.profile and state['profile_finished'] and recording:
            etl=guest_out+r'\control.etl'
            ssh(f"wpr -status collectors -details -instancename CapyWinuiPerf|Set-Content -LiteralPath {quote(guest_out+chr(92)+'wpr-status.txt')};wpr -stop {quote(etl)} -instancename CapyWinuiPerf;if($LASTEXITCODE){{throw 'WPR stop failed'}};wpr -setprofint {prior_interval}")
            recording=False;profile_finished=True
            ssh(f"[IO.File]::WriteAllText({quote(profile_dir+chr(92)+'stopped.json')},'{{}}')")
        if state['done'] and state['state'] not in ('Running','Queued'):
            break
        if time.monotonic()>deadline:
            raise RuntimeError(f'Owned task exceeded deadline: {task}')
        time.sleep(3)
    if args.profile and profile_finished and not args.record_only:
        etl=guest_out+r'\control.etl'
        ssh(fr"""$ErrorActionPreference='Stop'
$wpt=Join-Path ${{env:ProgramFiles(x86)}} 'Windows Kits\10\Windows Performance Toolkit'
$env:_NT_SYMBOL_PATH={quote(args.control_build+';C:'+chr(92)+'capy-bench'+chr(92)+'symbols-control')}
$csv=Join-Path {quote(guest_out)} 'csv'
New-Item -ItemType Directory -Force $csv|Out-Null
& (Join-Path $wpt 'xperf.exe') -i {quote(etl)} -o (Join-Path {quote(guest_out)} 'events.csv') -a dumper -stacktimeshifting
$export=Start-Process -FilePath (Join-Path $wpt 'wpaexporter.exe') -ArgumentList @('-i',('"'+{quote(etl)}+'"'),'-symbols','-profile','C:\capy-bench\harness\capy-ui.wpaProfile','-outputfolder',('"'+$csv+'"')) -PassThru -NoNewWindow -RedirectStandardOutput (Join-Path {quote(guest_out)} 'export.log') -RedirectStandardError (Join-Path {quote(guest_out)} 'export-stderr.log')
$null=$export.Handle;@{{process_id=$export.Id}}|ConvertTo-Json|Set-Content -LiteralPath (Join-Path {quote(guest_out)} 'export-process.json')
if(!$export.WaitForExit(120000)){{Stop-Process -Id $export.Id;throw 'Owned WPA export exceeded two minutes; ETL retained'}}
if($export.ExitCode -ne 0){{throw "WPA sampled CPU export failed: $($export.ExitCode)"}}
""")
    retrieve();retrieved=True
    ssh(f'Unregister-ScheduledTask -TaskName {quote(task)} -Confirm:$false')
    (local/'order.json').write_text(json.dumps({'order':args.order,'slots':slots,'workloads':workloads,'theme':args.theme},indent=2)+'\n')
    if state['exit_code']!=0 or json.loads((local/'task-result.json').read_text(encoding='utf-8-sig'))['exit_code']!=0:
        raise RuntimeError(f'Benchmark task failed; evidence: {local}')
except BaseException as error:
    failure=error
finally:
    try:
        if recording:
            ssh('wpr -cancel -instancename CapyWinuiPerf')
        if interval_changed:
            ssh(f"wpr -setprofint {prior_interval};if($LASTEXITCODE){{throw 'WPR interval restoration failed'}}")
    finally:
        try:
            if guest_created and not retrieved:
                try:
                    retrieve()
                except Exception as error:
                    print(f'Evidence retrieval failed for {guest_out}: {error}',file=sys.stderr)
        finally:
            if started:
                call('stop')
if failure:
    raise failure
print(f'Raw evidence: {local}',flush=True)
PY
