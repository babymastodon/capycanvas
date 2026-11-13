#!/usr/bin/env python3
"""Manage Windows evaluation VMs that build the Windows client and run its tests.

See docs/development/windows-vm.md.
"""

import argparse
import base64
from contextlib import contextmanager, nullcontext, ExitStack
from concurrent.futures import ThreadPoolExecutor, wait
import fcntl
import getpass
import hashlib
import json
import math
import os
from pathlib import Path
import re
from threading import Lock
import resource
import secrets
import shlex
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import time
from urllib.parse import unquote, urlparse

sys.path.insert(0, str(Path(__file__).resolve().parent))
import gpu_pool
import cpu_pool
import fixture_shards
import session_pool


ROOT = Path(__file__).resolve().parents[2]
GUEST_FILES = Path(__file__).resolve().parent
PROGRAM = sys.argv[0]
ISO_URL = "https://aka.ms/Win11E-ISO-25H2-en-us"
SERVER_ISO_URL = "https://aka.ms/WinServ2025iso-enus"
STATE = Path(os.environ.get("CAPYCANVAS_VM_DIR") or Path(
    os.environ.get("XDG_DATA_HOME") or Path.home() / ".local/share") / "capycanvas/windows-vm")
RUNTIME = Path(os.environ["CAPYCANVAS_VM_RUNTIME"]) if os.environ.get("CAPYCANVAS_VM_RUNTIME") else (
    Path(os.environ.get("XDG_RUNTIME_DIR") or tempfile.gettempdir()) /
    f"capycanvas-vm-{os.getuid()}-{hashlib.sha256(bytes(STATE)).hexdigest()[:8]}")
SETTINGS = gpu_pool.configuration(STATE)
CPUS = os.environ.get("CAPYCANVAS_VM_CPUS") or str(SETTINGS.get("cpus", min(8, os.cpu_count() or 1)))
MEMORY = os.environ.get("CAPYCANVAS_VM_MEMORY") or SETTINGS.get("memory", "16G")
ARTIFACTS = Path(os.environ.get("CAPYCANVAS_VM_ARTIFACTS") or SETTINGS.get("artifacts") or ROOT / "artifacts/windows-vm")
DISPLAY = os.environ.get("CAPYCANVAS_VM_DISPLAY") or "2560x1600"
VGA = "VGA,xres={},yres={},vgamem_mb=64".format(*DISPLAY.split("x"))
DISK_SIZE = "128G"
USER = "capy"
SEED_LABEL = "CAPYSEED"
GUEST_REPO = "C:\\capycanvas"
GUEST_PWSH = "C:\\Program Files\\PowerShell\\7\\pwsh.exe"
DESKTOP_TASK = "capycanvas-desktop"
SOFTWARE_BUILD = f"{GUEST_REPO}\\artifacts\\windows\\SoftwareAdapter"
HARDWARE_BUILD = f"{GUEST_REPO}\\artifacts\\windows\\Release"
ISO_RECORD = STATE / "iso"
FIRMWARE = STATE / "firmware.json"
KEY = STATE / "id_ed25519"
KNOWN_HOSTS = STATE / "known_hosts"
PASSWORD = STATE / "password"
INSTALL = STATE / "install"
BASE = STATE / "base"
SERVER_ISO_RECORD = STATE / "iso-server"
SERVER_PASSWORD = STATE / "password-server"
SERVER_INSTALL = STATE / "install-server"
SERVER_BASE = STATE / "base-server"
VMS = STATE / "vms"
LEGACY_RUN = STATE / "run"
LEGACY_PID = STATE / "qemu.pid"
VM_NAME = re.compile(r"[a-z0-9][a-z0-9-]{0,31}")
SHARED_PREFIX = "shared-gpu-"
SHARED_DISPLAY = "2560x1600"
SHARED_ROOT = "C:\\capyrun"
SHARED_OWNER = STATE / "shared-gpu"
RDP_CLIENT = STATE / "rdp-client/usr"
COMMANDS = ("qemu-system-x86_64", "qemu-img", "swtpm", "xorriso", "ssh", "ssh-keygen", "curl")
SERVER_PACKAGES = {
    "fedora": [["dnf", "install", "-y", "freerdp", "xorg-x11-server-Xvfb"]],
    "debian": [["apt-get", "install", "-y", "freerdp3-x11", "xvfb"]],
    "arch": [["pacman", "-S", "--needed", "--noconfirm", "freerdp", "xorg-server-xvfb"]],
}
PACKAGES = {
    "fedora": [["dnf", "install", "-y", "qemu-system-x86", "qemu-img", "qemu-ui-gtk", "edk2-ovmf",
                "swtpm", "xorriso", "openssh-clients", "curl"]],
    "debian": [["apt-get", "update"],
               ["apt-get", "install", "-y", "qemu-system-x86", "qemu-utils", "qemu-system-gui", "ovmf",
                "swtpm", "xorriso", "openssh-client", "curl"]],
    "arch": [["pacman", "-S", "--needed", "--noconfirm", "qemu-system-x86", "qemu-img", "qemu-ui-gtk",
              "edk2-ovmf", "swtpm", "libisoburn", "openssh", "curl"]],
}
HYPERV = ("hv-relaxed,hv-vapic,hv-spinlocks=0x1fff,hv-vpindex,hv-runtime,hv-synic,hv-stimer,"
          "hv-time,hv-frequencies,hv-tlbflush,hv-ipi")
CLEAR = (f"New-Item -ItemType Directory -Force {GUEST_REPO} | Out-Null; Get-ChildItem -Force {GUEST_REPO} | "
         "Where-Object Name -NotIn 'target', 'artifacts' | Remove-Item -Recurse -Force; ")
EXTRACT = f"tar -xmf - -C {GUEST_REPO}; exit $LASTEXITCODE"


class Machine:
    def __init__(self, name, directory, server=None):
        self.name = name
        self.directory = directory
        self.server = name.startswith(SHARED_PREFIX) if server is None else server
        self.base = SERVER_BASE if self.server else BASE
        self.runtime = RUNTIME / name
        self.qmp_socket = self.runtime / "qmp.sock"
        self.tpm_socket = self.runtime / "swtpm.sock"
        self.qemu_pid = self.runtime / "qemu.pid"
        self.tpm_pid = self.runtime / "swtpm.pid"
        self.ssh_port = self.runtime / "ssh-port"
        self.rdp_port = self.runtime / "rdp-port"
        self.prepared = directory / "prepared"
        self.gpu = directory / "gpu"
        self.runner = directory / "runner.json"


def installer():
    return Machine(".install", INSTALL)


def server_installer():
    return Machine(".install-server", SERVER_INSTALL, server=True)


def machines():
    return [Machine(path.name, path) for path in sorted(VMS.glob("*")) if VM_NAME.fullmatch(path.name)]


def selected(args):
    name = args.vm or os.environ.get("CAPYCANVAS_VM") or (
        re.sub(r"[^a-z0-9]+", "-", ROOT.name.lower()).strip("-")[:32] or "default")
    if not VM_NAME.fullmatch(name):
        sys.exit(f"VM names use lowercase letters, digits and hyphens, up to 32 characters: {name}")
    return Machine(name, VMS / name)


def run(*command, check=True, **options):
    return subprocess.run([str(part) for part in command], check=check, **options)


def remove(path):
    if path.is_dir():
        shutil.rmtree(path)
    else:
        path.unlink(missing_ok=True)


def write_private(path, text):
    flags = os.O_WRONLY | os.O_CREAT | os.O_NOFOLLOW
    with os.fdopen(os.open(path, flags, 0o600), "w") as output:
        os.fchmod(output.fileno(), 0o600)
        output.truncate()
        output.write(text)


def firmware():
    descriptors = {}
    for directory in ("/usr/share/qemu/firmware", "/etc/qemu/firmware"):
        descriptors.update((path.name, path) for path in Path(directory).glob("*.json"))
    candidates = []
    for name in sorted(descriptors):
        text = descriptors[name].read_text()
        description = json.loads(text) if text.strip() else {}
        mapping = description.get("mapping", {})
        features = description.get("features", [])
        if ("uefi" in description.get("interface-types", []) and mapping.get("device") == "flash"
                and "nvram-template" in mapping and "secure-boot" in features
                and any(target.get("architecture") == "x86_64"
                        and any(machine.startswith("pc-q35") for machine in target.get("machines", []))
                        for target in description.get("targets", []))):
            candidates.append(("enrolled-keys" not in features, mapping))
    if not candidates:
        return None
    mapping = min(candidates, key=lambda candidate: candidate[0])[1]
    return {"code": mapping["executable"]["filename"],
            "code_format": mapping["executable"].get("format", "raw"),
            "vars": mapping["nvram-template"]["filename"],
            "vars_format": mapping["nvram-template"].get("format", "raw")}


def ready():
    if not all(map(shutil.which, COMMANDS)):
        return False
    displays = run("qemu-system-x86_64", "-display", "help", capture_output=True, text=True).stdout
    return "gtk" in displays.split() and firmware() is not None


def package_family():
    release = dict(line.split("=", 1) for line in Path("/etc/os-release").read_text().splitlines()
                   if "=" in line)
    distributions = f"{release.get('ID', '')} {release.get('ID_LIKE', '')}".replace('"', "").split()
    return next((name for name in PACKAGES if name in distributions), None)


def install_packages():
    if ready():
        return
    family = package_family()
    if family is None:
        sys.exit("Install QEMU with its GTK display, OVMF, swtpm, xorriso, OpenSSH and curl, "
                 "then rerun setup.")
    sudo = [] if os.geteuid() == 0 else ["sudo"]
    for command in PACKAGES[family]:
        run(*sudo, *command)
    if not ready():
        sys.exit("QEMU, swtpm, xorriso, OpenSSH, curl or Secure Boot OVMF firmware is still missing.")


def rdp_tools():
    private = RDP_CLIENT / "bin"
    def tool(*names):
        return next((found for name in names for found in (shutil.which(name), private / name)
                     if found and Path(found).exists()), None)
    return tool("xfreerdp", "xfreerdp3"), tool("Xvfb")


def install_server_packages():
    if all(rdp_tools()):
        return
    family = package_family()
    if family is None:
        sys.exit("Install FreeRDP's X11 client and Xvfb, then rerun setup --server.")
    sudo = [] if os.geteuid() == 0 else ["sudo"]
    for command in SERVER_PACKAGES[family]:
        run(*sudo, *command)
    if not all(rdp_tools()):
        sys.exit("FreeRDP's X11 client or Xvfb is still missing.")


def ensure_kvm_access():
    kvm = Path("/dev/kvm")
    if not kvm.exists():
        sys.exit("/dev/kvm is missing; enable hardware virtualization and load the KVM module.")
    if not os.access(kvm, os.R_OK | os.W_OK):
        user = getpass.getuser()
        run("sudo", "usermod", "-aG", kvm.group(), user)
        sys.exit(f"Added {user} to the {kvm.group()} group. Log in again, then rerun setup.")


def downloads():
    if shutil.which("xdg-user-dir"):
        path = run("xdg-user-dir", "DOWNLOAD", capture_output=True, text=True).stdout.strip()
        if path and Path(path) != Path.home():
            return Path(path)
    return Path.home() / "Downloads"


def download_iso(source=ISO_URL):
    url = run("curl", "-fsSIL", "-o", "/dev/null", "-w", "%{url_effective}", source,
              capture_output=True, text=True).stdout
    name = unquote(Path(urlparse(url).path).name)
    if not name.endswith(".iso"):
        sys.exit(f"{source} did not redirect to an ISO: {url}")
    iso = downloads() / name
    if not iso.exists():
        iso.parent.mkdir(parents=True, exist_ok=True)
        partial = iso.with_name(f"{name}.part")
        run("curl", "-fL", "-C", "-", "-o", partial, url)
        partial.rename(iso)
    return iso


def setup(args):
    server = getattr(args, "server", False)
    install_packages()
    if server:
        install_server_packages()
    ensure_kvm_access()
    iso = args.iso.resolve() if args.iso else download_iso(SERVER_ISO_URL if server else ISO_URL)
    if not iso.is_file():
        sys.exit(f"{iso} is not a file.")
    STATE.mkdir(mode=0o700, parents=True, exist_ok=True)
    STATE.chmod(0o700)
    if shutil.which("chattr"):
        subprocess.run(["chattr", "+C", STATE], stderr=subprocess.DEVNULL)
    (SERVER_ISO_RECORD if server else ISO_RECORD).write_text(f"{iso}\n")
    print(f"Windows ISO: {iso}\nNext: {PROGRAM} create{' --server' if server else ''}")


def process(pidfile, name):
    try:
        pid = int(pidfile.read_text())
        command = Path(f"/proc/{pid}/cmdline").read_bytes().split(b"\0")
        owned = str(pidfile).encode() in command or f"file={pidfile}".encode() in command
        return pid if owned and Path(f"/proc/{pid}/comm").read_text().strip() == name else None
    except (FileNotFoundError, ValueError):
        return None


def running(machine):
    return process(machine.qemu_pid, "qemu-system-x86") is not None


def require_running(machine):
    if not running(machine):
        sys.exit(f"The {machine.name} VM is not running; run `{PROGRAM} --vm {machine.name} start`.")


def wait_for_exit(pid, timeout):
    deadline = time.monotonic() + timeout
    while Path(f"/proc/{pid}").exists():
        if time.monotonic() > deadline:
            return False
        time.sleep(1)
    return True


def terminate(pidfile, name):
    pid = process(pidfile, name)
    if pid:
        os.kill(pid, signal.SIGTERM)
        wait_for_exit(pid, 10)


def qmp(machine, command, **arguments):
    with socket.socket(socket.AF_UNIX) as connection:
        connection.connect(str(machine.qmp_socket))
        stream = connection.makefile("rw")
        stream.readline()
        for message in ({"execute": "qmp_capabilities"}, {"execute": command, "arguments": arguments}):
            stream.write(json.dumps(message) + "\n")
            stream.flush()
            reply = {}
            while not reply.keys() & {"return", "error"}:
                line = stream.readline()
                if not line:
                    return
                reply = json.loads(line)
            if "error" in reply:
                sys.exit(f"QEMU {command} failed: {reply['error']['desc']}")
        return reply["return"]


def forwarded_port(machine, guest_port):
    output = qmp(machine, "human-monitor-command", **{"command-line": "info usernet"})
    ports = re.findall(rf"^\s*TCP\[HOST_FORWARD\]\s+\d+\s+127\.0\.0\.1\s+(\d+)\s+10\.0\.2\.15\s+{guest_port}(?:\s|$)",
                       output or "", re.MULTILINE)
    if len(ports) != 1 or not 0 < int(ports[0]) <= 65535:
        sys.exit(f"QEMU did not report one bound port {guest_port} forward for {machine.name}.")
    return int(ports[0])


def launch(machine, cdroms=(), gui=False, gpu=None, reservations=(), cpus=None, memory=None):
    for directory in (RUNTIME, machine.runtime):
        directory.mkdir(mode=0o700, exist_ok=True)
    terminate(machine.tpm_pid, "swtpm")
    selected_firmware = json.loads(FIRMWARE.read_text())
    machine.ssh_port.unlink(missing_ok=True)
    run("swtpm", "socket", "--tpm2", "--tpmstate", f"dir={machine.directory / 'tpm'}",
        "--ctrl", f"type=unixio,path={machine.tpm_socket}", "--pid", f"file={machine.tpm_pid}",
        "--terminate", "--daemon")
    command = [
        "qemu-system-x86_64", "-name", f"capycanvas-windows-{machine.name.lstrip('.')}", "-nodefaults",
        "-machine", "q35,accel=kvm,smm=on", "-global", "driver=cfi.pflash01,property=secure,value=on",
        "-cpu", f"host,{HYPERV}", "-smp", f"cpus={cpus or CPUS},sockets=1,cores={cpus or CPUS},threads=1",
        "-m", memory or MEMORY, "-rtc", "base=utc",
        "-drive", f"if=pflash,unit=0,readonly=on,format={selected_firmware['code_format']},"
                  f"file={selected_firmware['code']}",
        "-drive", f"if=pflash,unit=1,format={selected_firmware['vars_format']},file={machine.directory / 'efivars'}",
        "-chardev", f"socket,id=tpm,path={machine.tpm_socket}", "-tpmdev", "emulator,id=tpm,chardev=tpm",
        "-device", "tpm-crb,tpmdev=tpm",
        "-drive", f"if=none,id=disk,format=qcow2,discard=unmap,file={machine.directory / 'disk.qcow2'}",
        "-device", "nvme,drive=disk,serial=capycanvas,bootindex=0",
        "-netdev", "user,id=net,hostfwd=tcp:127.0.0.1:0-:22" + (",hostfwd=tcp:127.0.0.1:0-:3389" if machine.server else ""),
        "-device", "e1000e,netdev=net",
        "-device", "qemu-xhci", "-device", "usb-tablet", "-device", VGA,
        "-qmp", f"unix:{machine.qmp_socket},server=on,wait=off", "-display", "gtk" if gui else "none",
        "-daemonize", "-pidfile", machine.qemu_pid,
    ]
    for index, image in enumerate(cdroms):
        command += ["-drive", f"if=none,id=cd{index},media=cdrom,readonly=on,file={image}",
                    "-device", f"ide-cd,drive=cd{index},bus=ide.{index},bootindex={index + 1}"]
    if gpu:
        command += ["-global", "q35-pcihost.pci-hole64-size=256G",
                    "-fw_cfg", "name=opt/ovmf/X-PciMmio64Mb,string=262144",
                    "-device", "pcie-root-port,id=gpuroot,bus=pcie.0,addr=0x10,chassis=1"]
        for function in gpu.functions:
            slot = function.rsplit(".", 1)[1]
            command += ["-device", f"vfio-pci,host={function},bus=gpuroot,addr=0x0.{slot},multifunction=on"]
        machine.gpu.write_text(gpu.address + "\n")
    else:
        machine.gpu.unlink(missing_ok=True)
    kind = "gpu" if gpu else "runner"
    for index, reservation in enumerate(reservations):
        command += ["-add-fd", f"fd={reservation.fileno()},set={99 + index},opaque=capycanvas-{kind}-{machine.name}"]
    try:
        run(*command, **({"pass_fds": tuple(lock.fileno() for lock in reservations)} if reservations else {}))
    except subprocess.CalledProcessError:
        terminate(machine.tpm_pid, "swtpm")
        raise
    if machine.qemu_pid.exists():
        machine.launched_pid = int(machine.qemu_pid.read_text())
    machine.ssh_port.write_text(f"{forwarded_port(machine, 22)}\n")
    if machine.server:
        machine.rdp_port.write_text(f"{forwarded_port(machine, 3389)}\n")


def host_alias(machine):
    return "capycanvas-windows-server" if getattr(machine, "server", False) else "capycanvas-windows-vm"


def ssh_command(machine, *remote):
    return ["ssh", "-i", str(KEY), "-p", machine.ssh_port.read_text().strip(), "-o", "IdentitiesOnly=yes",
            "-o", "BatchMode=yes", "-o", f"UserKnownHostsFile={KNOWN_HOSTS}",
            "-o", "StrictHostKeyChecking=accept-new", "-o", f"HostKeyAlias={host_alias(machine)}",
            "-o", "ConnectTimeout=30", "-o", "ServerAliveInterval=15", "-o", "ServerAliveCountMax=8",
            "-o", "LogLevel=ERROR", f"{USER}@127.0.0.1", *remote]


def guest(machine, command, **options):
    return run(*ssh_command(machine, command), **options)


def guest_script(machine, script, **options):
    encoded = base64.b64encode(script.encode("utf-16-le")).decode()
    payload = "\n".join(encoded[index:index + 1024] for index in range(0, len(encoded), 1024)) + "\n\n"
    loader = "$data=[Text.StringBuilder]::new(); while($line=[Console]::In.ReadLine()){$null=$data.Append($line)}; & ([ScriptBlock]::Create([Text.Encoding]::Unicode.GetString([Convert]::FromBase64String($data.ToString()))))"
    command = base64.b64encode(loader.encode("utf-16-le")).decode()
    text = options.get("text") or options.get("universal_newlines") or options.get("encoding") or options.get("errors")
    status = guest(machine, f"$input | powershell -NoProfile -NonInteractive -OutputFormat Text -EncodedCommand {command}; exit $LASTEXITCODE",
                   input=payload if text else payload.encode("ascii"), check=False, **options).returncode
    if status:
        sys.exit(f"A PowerShell script in the {machine.name} VM exited with status {status}.")


def wait_for_ssh(machine, timeout):
    deadline = time.monotonic() + timeout
    while run(*ssh_command(machine, "exit"), stdin=subprocess.DEVNULL, capture_output=True,
              check=False).returncode:
        if not running(machine):
            sys.exit("The VM stopped before SSH became available.")
        if time.monotonic() > deadline:
            sys.exit(f"SSH did not become available; inspect the VM with `{PROGRAM} screenshot`.")
        time.sleep(5)


def server_unattend(text):
    administrator = ("<AdministratorPassword><Value>@PASSWORD@</Value><PlainText>true</PlainText></AdministratorPassword>\n"
                     "        <LocalAccounts>")
    if text.count("<Value>1</Value>") != 1 or text.count("<LocalAccounts>") != 1:
        raise ValueError("The answer file no longer has one image index and one account list")
    return text.replace("<Value>1</Value>", "<Value>2</Value>").replace("<LocalAccounts>", administrator)


def create(args):
    server = getattr(args, "server", False)
    flag = " --server" if server else ""
    machine = server_installer() if server else installer()
    base, install = (SERVER_BASE, SERVER_INSTALL) if server else (BASE, INSTALL)
    record, password_file = (SERVER_ISO_RECORD, SERVER_PASSWORD) if server else (ISO_RECORD, PASSWORD)
    if (base / "disk.qcow2").exists():
        sys.exit(f"The base image already exists; run `{PROGRAM} destroy{flag}` to rebuild it.")
    if running(machine):
        sys.exit("A base image install is running.")
    iso = Path(record.read_text().strip()) if record.exists() else None
    if iso is None or not iso.is_file():
        sys.exit(f"The Windows ISO is missing; run `{PROGRAM} setup{flag}`.")
    selected_firmware = firmware()
    if selected_firmware is None:
        sys.exit(f"No Secure Boot OVMF firmware was found; run `{PROGRAM} setup`.")
    STATE.chmod(0o700)
    FIRMWARE.write_text(json.dumps(selected_firmware))
    remove(install)
    seed = install / "seed"
    (install / "tpm").mkdir(parents=True)
    seed.mkdir()
    shutil.copyfile(selected_firmware["vars"], install / "efivars")
    run("qemu-img", "create", "-q", "-f", "qcow2", install / "disk.qcow2", DISK_SIZE)
    if not KEY.exists():
        run("ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-C", "capycanvas-windows-vm", "-f", KEY)
    password = secrets.token_urlsafe(18) + ("aA1!" if server else "")
    write_private(password_file, f"{password}\n")
    unattend = (GUEST_FILES / "autounattend.xml").read_text()
    unattend = (server_unattend(unattend) if server else unattend).replace("@PASSWORD@", password)
    write_private(seed / "autounattend.xml", unattend)
    for name in ("bootstrap.ps1", "prepare.ps1", "password-policy.ps1"):
        shutil.copy(GUEST_FILES / name, seed)
    shutil.copy(GUEST_FILES / ("provision-server.ps1" if server else "provision.ps1"), seed / "provision.ps1")
    shutil.copy(KEY.with_suffix(".pub"), seed / "administrators_authorized_keys")
    image = run("xorriso", "-as", "mkisofs", "-quiet", "-J", "-joliet-long", "-r", "-V", SEED_LABEL,
                "-o", install / "seed.iso", seed, capture_output=True, text=True, check=False)
    if image.returncode:
        sys.exit(image.stderr)
    launch(machine, [iso, install / "seed.iso"], args.gui)
    print("Installing Windows and the build tools; this takes a while.", flush=True)
    for _ in range(30):
        qmp(machine, "send-key", keys=[{"type": "qcode", "data": "ret"}])
        time.sleep(1)
    wait_for_ssh(machine, 3 * 60 * 60)
    guest(machine, f"& ('{{0}}:\\provision.ps1' -f (Get-Volume -FileSystemLabel {SEED_LABEL}).DriveLetter)")
    stop_machine(machine)
    remove(seed)
    remove(install / "seed.iso")
    install.rename(base)
    (base / "disk.qcow2").chmod(0o444)
    print(f"Created the base image. Next: {PROGRAM} {'fixtures' if server else 'check'}")


def restart(machine):
    def booted():
        return guest(machine, "(Get-CimInstance Win32_OperatingSystem).LastBootUpTime.Ticks", stdin=subprocess.DEVNULL,
                     capture_output=True, text=True, check=False).stdout.strip()
    previous = booted()
    if not previous:
        sys.exit(f"Could not read the {machine.name} VM's boot time before restarting it.")
    guest(machine, "Restart-Computer -Force", check=False)
    deadline = time.monotonic() + 10 * 60
    while booted() in ("", previous):
        if time.monotonic() > deadline:
            sys.exit(f"The {machine.name} VM did not restart.")
        time.sleep(3)


def prepare_logon(machine):
    with tempfile.TemporaryFile() as output:
        guest_script(machine, (GUEST_FILES / "password-policy.ps1").read_text(), stdout=output)
        output.seek(0)
        state = json.load(output)
    if state["desktop_ready"]:
        return
    if state["password_expired"]:
        restart(machine)
    reason = "after repairing password expiry" if state["password_expired"] else "with a non-expired password"
    guest_script(machine, f"""$ErrorActionPreference='Stop'
$deadline=(Get-Date).AddSeconds(120)
while (!(Get-Process explorer -IncludeUserName -ErrorAction SilentlyContinue | Where-Object UserName -Like '*\\{USER}')) {{
    if ((Get-Date) -ge $deadline) {{ throw 'The private user interactive desktop did not start {reason}' }}
    Start-Sleep 1
}}
""", timeout=180)


def prepare(machine):
    prepare_logon(machine)
    script = (GUEST_FILES / "prepare.ps1").read_text()
    if machine.server:
        accounts = ",".join(f"'{account}'" for account in shared_accounts())
        script += (f"\n$sharedAccounts=@({accounts})\n$sharedPassword='{SERVER_PASSWORD.read_text().strip()}'\n"
                   + (GUEST_FILES / "prepare-shared.ps1").read_text())
    digest = hashlib.sha256(f"{VGA}\n{script}".encode()).hexdigest()
    if machine.prepared.exists() and machine.prepared.read_text().strip() == digest:
        return
    guest_script(machine, script)
    restart(machine)
    machine.prepared.write_text(f"{digest}\n")


def boot(machine, gui, gpu=None):
    machine.runtime.mkdir(mode=0o700, parents=True, exist_ok=True)
    with open(machine.runtime / "boot-lock", "a") as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            sys.exit(f"The {machine.name} VM is already starting.")
        boot_locked(machine, gui, gpu)


def boot_locked(machine, gui, gpu):
    runner = getattr(machine, "runner_owner", None)
    if machine.runner.exists():
        runner = json.loads(machine.runner.read_text())
    if runner and (runner.get("worktree") != str(ROOT.resolve()) or gpu):
        sys.exit("CPU runners belong to their creating worktree and cannot claim a GPU.")
    if running(machine):
        if gpu and not machine.gpu.exists():
            sys.exit(f"The {machine.name} VM is running without a reserved GPU; stop it before using --gpu.")
        if gpu and gpu != "auto" and machine.gpu.read_text().strip() != gpu:
            sys.exit(f"The {machine.name} VM already has another GPU; stop it before changing cards.")
        if machine.gpu.exists():
            prepare_gpu_display(machine)
        return
    if not (machine.base / "disk.qcow2").exists():
        sys.exit(f"The base image does not exist; run `{PROGRAM} create{' --server' if machine.server else ''}`.")
    if gpu and resource.getrlimit(resource.RLIMIT_MEMLOCK)[0] != resource.RLIM_INFINITY:
        sys.exit("GPU VMs require unlimited locked memory; use start --gpu after the host GPU setup.")
    with ExitStack() as reservations:
        device = None
        descriptors = ()
        cpus, memory = None, None
        if machine.server:
            slot, descriptors = reservations.enter_context(cpu_pool.reserve(
                STATE, machine.name, SHARED_OWNER / machine.name, slot_count=shared_settings()["cpu_slots"]))
            cpus, memory = slot.cpus, slot.memory
        elif runner or gpu and os.environ.get("CAPYCANVAS_VM_ROUND_GPU") == "1":
            slot, descriptors = reservations.enter_context(cpu_pool.reserve(
                STATE, machine.name, ROOT, slot_count=1 if runner else 2))
            cpus, memory = slot.cpus, slot.memory
            if gpu and (cpus, memory) != (8, "16G"):
                raise ValueError("Round GPU requires two 4-vCPU / 8-GiB CPU slots")
        if gpu:
            device, card = reservations.enter_context(gpu_pool.reserve(STATE, machine.name, gpu))
            descriptors = (card, *descriptors)
        if not machine.directory.exists():
            fresh = machine.directory.with_name(f"{machine.name}.partial")
            if runner and fresh.exists():
                sys.exit(f"An unfinished runner disk exists at {fresh}; inspect it before starting again.")
            remove(fresh)
            shutil.copytree(machine.base, fresh, ignore=shutil.ignore_patterns("disk.qcow2"))
            run("qemu-img", "create", "-q", "-f", "qcow2", "-F", "qcow2", "-b", machine.base / "disk.qcow2",
                fresh / "disk.qcow2")
            fresh.rename(machine.directory)
        if runner:
            machine.runner.write_text(json.dumps(runner, indent=2))
        launch(machine, gui=gui, gpu=device, reservations=descriptors, cpus=cpus, memory=memory)
    wait_for_ssh(machine, 10 * 60)
    prepare(machine)
    if gpu:
        prepare_gpu_driver(machine)
        record = prepare_gpu_display(machine)
        if os.environ.get("CAPYCANVAS_VM_ROUND_GPU") == "1":
            record = prepare_gpu_display(machine, fixture_scale=150)
            (machine.directory / "fixture-scale-change.json").write_text(json.dumps(record, indent=2))
            if record.get("restart_required"):
                restart(machine)
            record = prepare_gpu_display(machine)
            validate_round_display(record)


def prepare_gpu_driver(machine):
    driver = gpu_pool.configuration(STATE).get("guest_driver")
    if not driver:
        print("Install the vendor's Windows GPU driver in this VM before hardware tests.")
        return
    driver = Path(driver)
    manifests = sorted(path for path in driver.rglob("*") if path.suffix.lower() in (".inf", ".cat"))
    if not manifests or not any(path.suffix.lower() == ".inf" for path in manifests):
        sys.exit(f"The configured Windows GPU driver bundle is unavailable: {driver}")
    digest = hashlib.sha256()
    for path in manifests:
        digest.update(str(path.relative_to(driver)).encode())
        digest.update(path.read_bytes())
    version = digest.hexdigest()
    stamp = machine.directory / "gpu-driver"
    if stamp.exists() and stamp.read_text().strip() == version:
        return
    destination = f"C:\\capycanvas-gpu-driver\\{version}"
    guest(machine, f"New-Item -ItemType Directory -Force '{destination}' | Out-Null")
    archive = subprocess.Popen(["tar", "-cf", "-", "-C", str(driver), "."], stdout=subprocess.PIPE)
    try:
        status = guest(machine, f"tar -xf - -C {destination}", stdin=archive.stdout, check=False)
    finally:
        archive.stdout.close()
        archive_status = archive.wait()
    if archive_status or status.returncode:
        sys.exit("Could not stage the Windows GPU driver bundle")
    guest_script(machine, "$ErrorActionPreference='Stop'\n" + (GUEST_FILES / "guest-gpu.ps1").read_text() +
                 f"\nInstall-WindowsGpuDriver '{destination}'\n")
    guest(machine, f"Remove-Item -LiteralPath '{destination}' -Recurse -Force")
    restart(machine)
    stamp.write_text(version + "\n")


def prepare_gpu_display(machine, fixture_scale=None):
    width, height = map(int, DISPLAY.split("x"))
    directory = r"C:\capycanvas-vm-display"
    receipt = directory + r"\receipt.json"
    scale = f" -FixtureScale {fixture_scale}" if fixture_scale else ""
    task = "capycanvas-hardware-display" + ("-fixture" if fixture_scale else "")
    trigger = "" if fixture_scale else " -Trigger $trigger"
    with claim(machine):
        guest_script(machine, f"""$ErrorActionPreference='Stop'
$ProgressPreference='SilentlyContinue'
$task='{task}'
$receipt='{receipt}'
$deadline=(Get-Date).AddSeconds(120)
while (!(Get-Process explorer -IncludeUserName -ErrorAction SilentlyContinue | Where-Object UserName -Like '*\\{USER}')) {{
    if ((Get-Date) -ge $deadline) {{ throw 'The interactive desktop did not start' }}
    Start-Sleep 1
}}
while ((Get-ScheduledTask $task -ErrorAction SilentlyContinue).State -eq 'Running') {{
    if ((Get-Date) -ge $deadline) {{ throw 'Hardware display task is still running' }}
    Start-Sleep 1
}}
""", timeout=180)
        guest(machine, f"New-Item -ItemType Directory -Force '{directory}' | Out-Null")
        for name in ("hardware-display.cs", "hardware-display.ps1"):
            content = base64.b64encode((GUEST_FILES / name).read_bytes()).decode("ascii")
            guest_script(machine, f"[IO.File]::WriteAllBytes('{directory}\\{name}', "
                         f"[Convert]::FromBase64String('{content}'))")
        guest_script(machine, f"""$ErrorActionPreference='Stop'
$ProgressPreference='SilentlyContinue'
$task='{task}'
$receipt='{receipt}'
$deadline=(Get-Date).AddSeconds(120)
Remove-Item -LiteralPath $receipt -Force -ErrorAction SilentlyContinue
$action=New-ScheduledTaskAction -Execute '{GUEST_PWSH}' -Argument '-NoProfile -WindowStyle Hidden -File {directory}\\hardware-display.ps1 -Width {width} -Height {height} -Receipt {receipt}{scale}'
$principal=New-ScheduledTaskPrincipal -UserId {USER} -LogonType Interactive -RunLevel Highest
$trigger=New-ScheduledTaskTrigger -AtLogOn -User {USER}
$settings=New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit (New-TimeSpan -Minutes 3) -RestartCount 3 -RestartInterval (New-TimeSpan -Minutes 1)
Register-ScheduledTask $task -Action $action -Principal $principal{trigger} -Settings $settings -Force | Out-Null
Start-ScheduledTask $task
while (!(Test-Path -LiteralPath $receipt)) {{
    if ((Get-Date) -ge $deadline) {{ throw 'Hardware display receipt timed out' }}
    Start-Sleep 1
}}
$result=Get-Content -LiteralPath $receipt -Raw|ConvertFrom-Json
if (!$result.success) {{ throw $result.error }}
$result|ConvertTo-Json -Depth 5
""", timeout=180)
        result = guest(machine, f"Get-Content -LiteralPath '{receipt}' -Raw", capture_output=True, text=True)
        record = json.loads(result.stdout)
        display = record.get("display", {})
        if not record.get("success") or not display.get("primary") or "NVIDIA" not in display.get("adapter", "") or (
                display.get("width"), display.get("height"), display.get("refreshHz")) != (width, height, 120):
            sys.exit("Hardware desktop receipt does not match the requested NVIDIA primary mode.")
        (machine.directory / "display.json").write_text(json.dumps(record, indent=2))
        return record


def start(args):
    machine = selected(args)
    if machine.server:
        start_shared_vm(machine)
        print(f"Starting the {machine.name} VM; `{PROGRAM} sessions` shows when it is ready.")
        return
    gpu = getattr(args, "gpu", None)
    if gpu and not running(machine) and not os.environ.get("CAPYCANVAS_VM_GPU_SERVICE"):
        start_gpu_service(machine, args)
        return
    try:
        boot(machine, args.gui, gpu)
    except (Exception, SystemExit) as error:
        if not gpu or not os.environ.get("CAPYCANVAS_VM_GPU_SERVICE"):
            raise
        (machine.runtime / "startup-error").write_text(str(error))
        if not running(machine):
            raise
        print(f"GPU VM startup check failed: {error}; the VM remains running for diagnosis.", file=sys.stderr)
        return
    print(f"The {machine.name} VM is running. Connect with `{PROGRAM} --vm {machine.name} ssh`.")


def gpu_unit(machine):
    return f"capy-vm-{hashlib.sha256(bytes(STATE)).hexdigest()[:8]}-{machine.name}"


def start_gpu_service(machine, args):
    machine.runtime.mkdir(mode=0o700, parents=True, exist_ok=True)
    with open(machine.runtime / "service-lock", "a") as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            sys.exit(f"The {machine.name} GPU VM is already starting.")
        if running(machine):
            sys.exit(f"The {machine.name} VM started in another process; inspect it before retrying.")
        start_gpu_service_locked(machine, args)


def start_gpu_service_locked(machine, args):
    error = machine.runtime / "startup-error"
    error.unlink(missing_ok=True)
    unit = gpu_unit(machine)
    run("systemctl", "--user", "stop", unit, check=False, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    program = [sys.executable, str(Path(__file__).resolve()), "--vm", machine.name]
    command = ["systemd-run", "--user", f"--unit={unit}", "--collect",
               "--property=Type=oneshot", "--property=RemainAfterExit=yes",
               "--property=KillMode=process", "--property=LimitMEMLOCK=infinity",
               "--property=TimeoutStartSec=45min",
               "--property=TimeoutStopSec=6min",
               f"--property=ExecStop={shlex.join([item.replace('$', '$$') for item in [*program, 'stop']])}"]
    environment = {"CAPYCANVAS_VM_GPU_SERVICE": "1", "CAPYCANVAS_VM_DIR": str(STATE),
                   "CAPYCANVAS_VM_RUNTIME": str(RUNTIME), "CAPYCANVAS_VM_CPUS": CPUS,
                   "CAPYCANVAS_VM_MEMORY": MEMORY, "CAPYCANVAS_VM_DISPLAY": DISPLAY}
    if getattr(args, "round_gpu", False):
        environment["CAPYCANVAS_VM_ROUND_GPU"] = "1"
    command += [f"--setenv={key}={value}" for key, value in environment.items()]
    command += [*[item.replace("$", "$$") for item in program], "start", "--gpu", args.gpu]
    if args.gui:
        command.append("--gui")
    status = run(*command, check=False)
    if error.exists():
        sys.exit(error.read_text())
    if status.returncode:
        sys.exit(f"The {machine.name} GPU VM did not start; see `journalctl --user -u {unit}`.")


def stop_machine(machine):
    pid = process(machine.qemu_pid, "qemu-system-x86")
    if pid is None:
        return
    qmp(machine, "system_powerdown")
    if not wait_for_exit(pid, 5 * 60):
        if machine.gpu.exists():
            sys.exit("The GPU VM did not shut down cleanly; it and its GPU reservation remain running.")
        qmp(machine, "quit")
        if not wait_for_exit(pid, 30):
            raise RuntimeError(f"The {machine.name} VM is still running after shutdown; its reservation remains held.")


def stop(args):
    machine = selected(args)
    if machine.server:
        with shared_pool().closed(machine.name, shared_accounts()) as closed:
            if not closed:
                sys.exit(f"Fixture runs hold sessions on {machine.name}; it stops by itself when they finish.")
            run("systemctl", "--user", "stop", shared_unit(machine), check=False,
                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            stop_machine(machine)
        return
    stop_machine(machine)
    if not os.environ.get("CAPYCANVAS_VM_GPU_SERVICE"):
        run("systemctl", "--user", "stop", gpu_unit(machine), check=False,
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def gpus(args):
    rows = gpu_pool.describe(STATE)
    if not rows:
        print("No GPU pool configured; see docs/development/windows-vm.md.")
    for row in rows:
        owner = row["owner"]
        detail = f" ({owner['vm']} in {owner['state']})" if owner else ""
        print(f"{row['address']}\t{row['status']}{detail}")


def runner_machine(primary, index):
    suffix = f"-r{index}"
    prefix = primary.name
    if len(prefix + suffix) > 32:
        prefix = prefix[:23 - len(suffix)] + "-" + hashlib.sha256(primary.name.encode()).hexdigest()[:8]
    name = prefix + suffix
    machine = Machine(name, VMS / name)
    if machine.directory.exists() and not machine.runner.exists():
        sys.exit(f"The existing {name} VM is not an owned CPU runner.")
    machine.runner_owner = {"worktree": str(ROOT.resolve()), "primary": primary.name}
    return machine


def runners(args):
    if args.reset:
        primary = selected(args)
        owned = [machine for machine in machines() if machine.runner.exists()
                 and json.loads(machine.runner.read_text()) == {"worktree": str(ROOT.resolve()), "primary": primary.name}]
        with ExitStack() as locks:
            for machine in owned:
                machine.runtime.mkdir(mode=0o700, parents=True, exist_ok=True)
                boot_lock = locks.enter_context(open(machine.runtime / "boot-lock", "a"))
                try:
                    fcntl.flock(boot_lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
                except BlockingIOError:
                    sys.exit(f"The {machine.name} runner is starting; stop it before resetting.")
                locks.enter_context(claim(machine))
                if running(machine) or runner_reserved(machine):
                    sys.exit(f"Stop the {machine.name} runner before resetting it.")
            for machine in owned:
                remove(machine.directory)
                print(f"Reset {machine.name}")
    settings, rows = cpu_pool.describe(STATE)
    print(f"CPU runner budget: {settings['vcpu_budget']} vCPU / {settings['memory_budget_gib']} GiB; "
          f"runner {settings['runner_cpus']} vCPU / {settings['runner_memory_gib']} GiB; "
          f"per owner {settings['per_owner_limit']}")
    for row in rows:
        owner = row["owner"]
        detail = f" {owner['vm']} in {owner['worktree']} since {owner['started']}" if owner else ""
        print(f"{row['slot']}\t{row['status']}{detail}")
    print(f"{sum(row['status'] == 'free' for row in rows)} free, "
          f"{sum(row['status'] == 'reserved' for row in rows)} reserved")


def runner_reserved(machine):
    return any(row["owner"] and row["owner"].get("vm") == machine.name
               and row["owner"].get("state") == str(STATE.resolve()) for row in cpu_pool.describe(STATE)[1])


def ssh(args):
    machine = selected(args)
    require_running(machine)
    os.execvp("ssh", ssh_command(machine, *args.command))


def claim(machine):
    directory = machine.directory.resolve()
    directory.parent.mkdir(parents=True, exist_ok=True)
    lock = cpu_pool.lock_file(directory.with_name("." + directory.name + ".lock"))
    if lock is None:
        sys.exit(f"Another command is using the {machine.name} VM; pass --vm to use another one.")
    return lock


def sync_machine(machine):
    require_running(machine)
    listing = run("git", "ls-files", "-z", "--cached", "--others", "--exclude-standard", "--exclude=target",
                  cwd=ROOT, capture_output=True).stdout
    hashes = {}
    for name in listing.decode().split("\0"):
        path = ROOT / name
        if name and os.path.lexists(path):
            content = os.readlink(path).encode() if path.is_symlink() else path.read_bytes()
            hashes[name] = hashlib.sha256(content).hexdigest()
    record = machine.directory / "synced.json"
    synced = json.loads(record.read_text()) if record.exists() else {}
    full = not synced or not synced.keys() <= hashes.keys()
    files = "".join(f"{name}\0" for name, digest in hashes.items() if full or synced.get(name) != digest)
    record.unlink(missing_ok=True)
    archive = subprocess.Popen(["tar", "--null", "--files-from=-", "--create", "--file=-"], cwd=ROOT,
                               stdin=subprocess.PIPE, stdout=subprocess.PIPE)
    extract = subprocess.Popen(ssh_command(machine, (CLEAR if full else "") + EXTRACT), stdin=archive.stdout)
    archive.stdout.close()
    archive.stdin.write(files.encode())
    archive.stdin.close()
    if archive.wait() or extract.wait():
        sys.exit("Copying the working tree to the VM failed.")
    record.write_text(json.dumps(hashes))
    return hashes


def sync(args):
    machine = selected(args)
    with claim(machine):
        sync_machine(machine)


def check(args):
    machine = selected(args)
    boot(machine, gui=False)
    with claim(machine):
        sync_machine(machine)
        prepare_rust(machine)
        configuration = "Debug" if args.debug else "Release"
        name = "check-" + secrets.token_hex(16)
        root = f"{GUEST_REPO}\\artifacts\\windows\\vm-check"
        output = root + "\\" + name
        task = "capycanvas-" + name
        guest_script(machine, f"""$ErrorActionPreference='Stop'
[IO.Directory]::CreateDirectory('{output}')|Out-Null
@'
$ErrorActionPreference='Stop'
$identity=@{{process_id=$PID;process_start_utc=(Get-Process -Id $PID).StartTime.ToUniversalTime().ToString('o')}}
[IO.File]::WriteAllText('{output}\\started.tmp',($identity|ConvertTo-Json))
[IO.File]::Move('{output}\\started.tmp','{output}\\started.json')
$ErrorActionPreference='Continue'
& '{GUEST_PWSH}' -NoProfile -File '{GUEST_REPO}\\apps\\layer-windows\\scripts\\test-without-gpu.ps1' -Configuration {configuration} 1> '{output}\\stdout.log' 2> '{output}\\stderr.log'
$code=$LASTEXITCODE
$ErrorActionPreference='Stop'
if($null -eq $code){{throw 'The Windows check process has no exit status.'}}
$identity.exit_code=$code
[IO.File]::WriteAllText('{output}\\result.tmp',($identity|ConvertTo-Json))
[IO.File]::Move('{output}\\result.tmp','{output}\\result.json')
exit $code
'@|Set-Content -LiteralPath '{output}\\check.ps1' -Encoding UTF8
""")
        failure = None
        try:
            desktop(machine, f'-File "{output}\\check.ps1"', task=task)
            startup_deadline = state_deadline = time.monotonic() + 180
            while True:
                status = guest(machine, f"""$ErrorActionPreference='Stop'
$task=Get-ScheduledTask -TaskName '{task}'
$info=Get-ScheduledTaskInfo -TaskName '{task}'
$result=if(Test-Path -LiteralPath '{output}\\result.json'){{[IO.File]::ReadAllText('{output}\\result.json')|ConvertFrom-Json}}else{{$null}}
@{{state=[string]$task.State;task_result=[long]$info.LastTaskResult;started=(Test-Path -LiteralPath '{output}\\started.json');result=$result}}|ConvertTo-Json -Depth 4 -Compress
""", capture_output=True, text=True, timeout=60)
                progress = json.loads(status.stdout)
                if progress["state"] not in ("Running", "Queued") and progress["task_result"] not in (267009, 267011):
                    if progress["result"] is None:
                        raise RuntimeError(f"The Windows check task stopped without a terminal receipt (task result {progress['task_result']}).")
                    code = progress["result"]["exit_code"]
                    if code:
                        sys.exit(code)
                    break
                now = time.monotonic()
                if progress["result"] is None and progress["state"] in ("Running", "Queued"):
                    state_deadline = now + 180
                if (not progress["started"] and now >= startup_deadline) or now >= state_deadline:
                    raise TimeoutError("The interactive Windows check task did not publish its startup or terminal receipt within three minutes.")
                time.sleep(5)
        except BaseException as error:
            failure = error
        finally:
            try:
                local = retrieve(machine, root, name)
                for filename, stream in (("stdout.log", sys.stdout), ("stderr.log", sys.stderr)):
                    path = local / filename
                    if path.exists():
                        with path.open("rb") as source:
                            shutil.copyfileobj(source, stream.buffer)
                        stream.flush()
            except BaseException as error:
                if failure is None:
                    failure = error
                else:
                    print(f"Could not retrieve Windows check evidence: {error}", file=sys.stderr)
            try:
                guest(machine, f"""$ErrorActionPreference='Stop'
$task=Get-ScheduledTask -TaskName '{task}' -ErrorAction SilentlyContinue
if($task){{
    $info=Get-ScheduledTaskInfo -TaskName '{task}'
    if($task.State -notin 'Running','Queued' -and $info.LastTaskResult -notin 267009,267011){{Unregister-ScheduledTask -TaskName '{task}' -Confirm:$false}}
}}
""", timeout=60)
            except BaseException as error:
                if failure is None:
                    failure = error
                else:
                    print(f"Could not unregister the completed Windows check task: {error}", file=sys.stderr)
        if failure is not None:
            raise failure


def desktop(machine, arguments, user=USER, task=DESKTOP_TASK, directory=GUEST_REPO):
    guest_script(machine, f"""$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$deadline = (Get-Date).AddMinutes(2)
while (!(Get-Process explorer -IncludeUserName -ErrorAction SilentlyContinue | Where-Object UserName -Like '*\\{user}')) {{
    if ((Get-Date) -ge $deadline) {{ throw 'The interactive desktop did not start.' }}
    Start-Sleep 2
}}
$action = New-ScheduledTaskAction -Execute conhost.exe -Argument '"{GUEST_PWSH}" -NoProfile -Sta -ExecutionPolicy Bypass {arguments}' -WorkingDirectory '{directory}'
$principal = New-ScheduledTaskPrincipal -UserId {user} -LogonType Interactive -RunLevel Limited
$settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit 0
Register-ScheduledTask {task} -Action $action -Principal $principal -Settings $settings -Force | Out-Null
Start-ScheduledTask {task}
""", timeout=180)


def build_inputs(hashes):
    scripts = {"apps/layer-windows/scripts/build.ps1", "apps/layer-windows/scripts/stage-assets.ps1",
               "apps/layer-windows/scripts/WindowsArchitecture.ps1"}
    return {name: digest for name, digest in hashes.items()
            if name in ("Cargo.toml", "Cargo.lock", "rust-toolchain.toml") or name in scripts
            or name.startswith(("crates/", "vendor/", ".cargo/")) and not name.endswith(".md")
            or name.startswith(("apps/layer-web/icons/", "apps/layer-web/brush-previews/",
                                "apps/layer-web/filter-previews/", "assets/locales/"))
            or name.startswith("apps/layer-windows/") and not name.endswith((".md", ".ps1"))
            and not name.startswith("apps/layer-windows/scripts/")}


def build_files(machine, directory=SOFTWARE_BUILD):
    result = guest(machine, f"$ErrorActionPreference='Stop'; $build='{directory}'; "
                   "ConvertTo-Json -Compress -InputObject @(Get-ChildItem $build -Recurse -File | "
                   "Where-Object { $_.Extension -In '.exe','.dll','.pri','.winmd','.manifest','.xbf','.mui','.xml','.dat' "
                   "-or $_.Name -match '\\.(deps|runtimeconfig)\\.json$' -or $_.FullName.StartsWith($build+'\\Assets\\') } | "
                   "ForEach-Object { @{name=$_.FullName.Substring($build.Length+1);"
                   "sha256=(Get-FileHash -LiteralPath $_.FullName).Hash} })",
                   capture_output=True, text=True, timeout=60)
    return {item["name"]: item["sha256"] for item in json.loads(result.stdout)}


def validate_build(record, inputs, files, profile="release"):
    if not record or record.get("profile") != profile or record["inputs"] != inputs or record["files"] != files:
        sys.exit("The build does not match the synced source or binaries; rerun without --no-build.")


def build_record_path(machine, hardware, profile):
    suffix = "" if profile == "release" else f"-{profile}"
    return machine.directory / f"{'hardware' if hardware else 'software'}-build{suffix}.json"


def publish_build_record(machine, directory, record):
    payload = base64.b64encode(json.dumps(record).encode()).decode()
    guest_script(machine, f"[IO.File]::WriteAllBytes('{directory}\\fixture-build.json',"
                 f"[Convert]::FromBase64String('{payload}'))")


def validate_controls(machine, directory, record, hardware, required):
    controls = record.get("controls", {})
    if not set(required) <= set(controls) or record.get("hardware_gpu") != hardware:
        sys.exit("Missing control-fixture build record; rerun without --no-build.")
    for kind in required:
        files = controls[kind]
        if not files or files != build_files(machine, directory + "-" + kind):
            sys.exit(f"The {kind} control fixture does not match its build record; rerun without --no-build.")


def fixture_directory(hardware, profile):
    return (HARDWARE_BUILD if hardware else SOFTWARE_BUILD) + ("" if profile == "release" else f"-{profile}")


def prepare_rust(machine):
    script = (GUEST_FILES / "prepare-rust.ps1").read_text()
    guest_script(machine, f"$ErrorActionPreference='Stop'; Set-Location '{GUEST_REPO}'\n"
                 f"& {{\n{script}\n}} -Manifest '{GUEST_REPO}\\Cargo.toml'")


def fixture_build(machine, hashes, no_build, hardware, profile="release", controls=None):
    prepare_rust(machine)
    inputs = build_inputs(hashes)
    directory = fixture_directory(hardware, profile)
    intermediate = (HARDWARE_BUILD if hardware else SOFTWARE_BUILD) + ".obj"
    build_record = build_record_path(machine, hardware, profile)
    if no_build:
        record = json.loads(build_record.read_text()) if build_record.exists() else None
        validate_build(record, inputs, build_files(machine, directory), profile)
        if controls is not None:
            validate_controls(machine, directory, record, hardware, controls)
    else:
        build_record.unlink(missing_ok=True)
        feature = "" if hardware else "-SoftwareAdapterTests "
        guest(machine, f"& '{GUEST_PWSH}' -NoProfile -File '{GUEST_REPO}\\apps\\layer-windows\\scripts\\build.ps1' -Configuration Release "
                       f"-RustProfile {profile} {feature}-OutputDirectory '{directory}' -IntermediateDirectory '{intermediate}'")
        record = {"profile": profile, "inputs": inputs, "files": build_files(machine, directory)}
        if controls is not None:
            record.update(controls={}, hardware_gpu=hardware)
            for kind in controls:
                target = directory + "-" + kind
                guest(machine, f"& '{GUEST_PWSH}' -NoProfile -File '{GUEST_REPO}\\apps\\layer-windows\\scripts\\build.ps1' "
                      f"-Configuration Release -RustProfile {profile} {feature}-SkipRust -SkipRestore -ControlFixture {kind} "
                      f"-OutputDirectory '{target}' -IntermediateDirectory '{intermediate}'")
                record["controls"][kind] = build_files(machine, target)
        build_record.write_text(json.dumps(record))
    if controls is not None:
        record = dict(record, controls={kind: record["controls"][kind] for kind in controls})
        publish_build_record(machine, directory, record)
    return directory, record


def retrieve(machine, root, name, local=None):
    local = local or ARTIFACTS / machine.name
    local.mkdir(parents=True, exist_ok=True)
    fetch = subprocess.Popen(ssh_command(machine, f"tar -cf - -C {root} {name}"), stdout=subprocess.PIPE)
    run("tar", "-xf", "-", "-C", local, stdin=fetch.stdout)
    fetch.stdout.close()
    if fetch.wait():
        sys.exit("Could not retrieve the VM test evidence.")
    return local / name


def gpu_check(args):
    start(argparse.Namespace(vm=args.vm, gui=False, gpu="auto"))
    machine = selected(args)
    with claim(machine):
        revision = run("git", "rev-parse", "HEAD", cwd=ROOT, capture_output=True, text=True).stdout.strip()
        hashes = sync_machine(machine)
        directory, record = fixture_build(machine, hashes, args.no_build, True)
        name = "gpu-" + time.strftime("%Y%m%d-%H%M%S")
        root = f"{GUEST_REPO}\\artifacts\\windows\\vm-gpu-tests"
        status = guest(machine, f"& '{GUEST_PWSH}' -NoProfile -File '{GUEST_REPO}\\tools\\windows-vm\\test-with-gpu.ps1' "
                       f"-Configuration Release -Output '{root}\\{name}' -BuildDirectory '{directory}'", check=False)
        local = retrieve(machine, root, name)
        (local / "provenance.json").write_text(json.dumps({"revision": revision, "source": hashes, "build": record}, indent=2))
        print(f"Hardware GPU test evidence: {local}")
        if status.returncode:
            sys.exit(f"Hardware GPU tests failed (exit {status.returncode}); see {local}")


def validate_results(plan, results):
    expected = plan["runs"]
    actual = [row["name"] for row in results]
    if not expected or actual != expected:
        sys.exit(f"Incomplete fixture coverage: expected {expected}, received {actual}.")


FIXTURE_RUNNER = r"""function Get-FixtureRunner([string]$Output){
    $identity=[IO.File]::ReadAllText((Join-Path $Output 'runner.json'))|ConvertFrom-Json
    Get-Process -Id $identity.process_id -ErrorAction SilentlyContinue|Where-Object {$_.StartTime.ToUniversalTime() -eq ([DateTime]$identity.process_start_utc).ToUniversalTime()}
}
"""


def fixture_status_command(output):
    return (r"""$ErrorActionPreference='Stop'
[Console]::OutputEncoding=[Text.UTF8Encoding]::new($false)
function Read-FixtureText([string]$Path){
    $stream=[IO.File]::Open($Path,[IO.FileMode]::Open,[IO.FileAccess]::Read,([IO.FileShare]::ReadWrite -bor [IO.FileShare]::Delete))
    $reader=[IO.StreamReader]::new($stream)
    try{$reader.ReadToEnd()}finally{$reader.Dispose();$stream.Dispose()}
}
""" + FIXTURE_RUNNER + r"""$runner=$null;if(Test-Path '{output}\runner.json'){$runner=[bool](Get-FixtureRunner '{output}')}
@{runner=$runner;
plan=$(if(Test-Path '{output}\plan.json'){Read-FixtureText '{output}\plan.json'|ConvertFrom-Json});
log=$(if(Test-Path '{output}\results.jsonl'){Read-FixtureText '{output}\results.jsonl'});
error=$(if(Test-Path '{output}\runner-error.txt'){Read-FixtureText '{output}\runner-error.txt'});
complete=(Test-Path '{output}\complete')}|ConvertTo-Json -Depth 5 -Compress
""").replace("{output}", output)


def stop_fixture_runner(machine, output):
    guest(machine, FIXTURE_RUNNER + fr"""$ErrorActionPreference='Stop'
if(Test-Path '{output}\runner.json'){{
    $runner=Get-FixtureRunner '{output}'
    if($runner){{
        & taskkill.exe /T /F /PID $runner.Id|Out-Null
        if(!$runner.WaitForExit(30000)){{throw "The fixture runner $($runner.Id) survived its stop."}}
    }}
}}
""", capture_output=True, text=True, timeout=60)


def fixture_progress(machine, output):
    command = fixture_status_command(output)
    polled = time.monotonic()
    try:
        status = guest(machine, command, capture_output=True, text=True, timeout=60)
    except subprocess.TimeoutExpired as error:
        elapsed = time.monotonic() - polled
        try:
            guest(machine, "exit 0", stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=20)
        except (subprocess.TimeoutExpired, subprocess.CalledProcessError):
            raise RuntimeError(f"{machine.name} stopped responding to SSH (status timeout {error.timeout} s)")
        evidence = ARTIFACTS / machine.name / output.rsplit("\\", 1)[-1]
        evidence.mkdir(parents=True, exist_ok=True)
        with (evidence / "slow-status.jsonl").open("a") as record:
            record.write(json.dumps({"utc": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
                                     "command": command, "elapsed": elapsed}) + "\n")
        return {"runner": None, "plan": None, "log": None, "error": None, "complete": False}
    try:
        return json.loads(status.stdout)
    except json.JSONDecodeError as error:
        raise RuntimeError(f"Invalid fixture progress JSON: {error}; stdout={status.stdout!r}; stderr={status.stderr!r}") from error


def wait_for_fixtures(machine, output, drain=None):
    results = []
    deadline = time.monotonic() + 180
    plan = None
    failure = None
    try:
        while True:
            time.sleep(5)
            progress = fixture_progress(machine, output)
            if progress["plan"]:
                if plan is None:
                    deadline = time.monotonic() + progress["plan"]["timeout_minutes"] * 60 + 120
                plan = progress["plan"]
            for line in (progress["log"] or "").splitlines()[len(results):]:
                try:
                    result = json.loads(line)
                except json.JSONDecodeError:
                    break
                results.append(result)
                deadline = time.monotonic() + plan["timeout_minutes"] * 60 + 120
                print(f"{'pass' if result['exit'] == 0 else 'FAIL'} {result['seconds']:5.0f}s {result['name']}", flush=True)
            if progress["complete"]:
                break
            if time.monotonic() >= deadline:
                raise TimeoutError("The fixture runner exceeded its startup or fixture deadline.")
            if progress["runner"] is False:
                raise RuntimeError(progress.get("error") or "The desktop task stopped before completing the fixture plan.")
    except (RuntimeError, TimeoutError, ValueError, TypeError, subprocess.SubprocessError) as error:
        failure = str(error)
        if isinstance(error, subprocess.CalledProcessError):
            failure += f"; stdout={error.stdout!r}; stderr={error.stderr!r}"
        if drain:
            drain()
        try:
            stop_fixture_runner(machine, output)
        except subprocess.SubprocessError as error:
            failure += f"; the fixture runner did not confirm its stop: {error}; stderr={getattr(error, 'stderr', None)!r}"
    return results, plan, failure


def fixture_selection(names):
    if any(not re.fullmatch(r"[a-zA-Z0-9:-]+", item) for item in names):
        raise ValueError("Fixture names use letters, numbers, hyphens and a variant colon.")
    return f" -Name {','.join(names)}" if names else ""


@contextmanager
def fixture_local_dumps(machine, output, cleanup):
    definitions = f"""$ErrorActionPreference='Stop'
$Output='{output}'
$tokens=$null;$errors=$null
$ast=[Management.Automation.Language.Parser]::ParseFile('{GUEST_REPO}\\tools\\windows-vm\\run-fixtures.ps1',[ref]$tokens,[ref]$errors)
if($errors.Count){{throw ($errors|Out-String)}}
foreach($name in @('Get-FixtureLocalDumps','Enable-FixtureLocalDumps','Restore-FixtureLocalDumps')){{
 $definition=$ast.Find({{param($node)$node -is [Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq $name}},$false)
 if(!$definition){{throw "Missing actual WER helper: $name"}}
 . ([scriptblock]::Create($definition.Extent.Text))
}}
"""
    try:
        guest_script(machine, definitions + "Enable-FixtureLocalDumps (Get-FixtureLocalDumps)")
        yield
    finally:
        try:
            guest_script(machine, definitions + """$backup=Join-Path $Output 'wer/registry-before.clixml'
if(Test-Path -LiteralPath $backup){Restore-FixtureLocalDumps (Import-Clixml -LiteralPath $backup)}
""")
        except (Exception, SystemExit) as error:
            cleanup.append(str(error))


def execute_fixtures(machine, args, directory, provenance, names, queue=None, drain=None):
    name = time.strftime("%Y%m%d-%H%M%S")
    output = f"{GUEST_REPO}\\artifacts\\windows\\vm-fixtures\\{name}"
    selection = fixture_selection(names)
    mode = " -HardwareGpu" if provenance["hardware_gpu"] else ""
    mode += f" -QueueConfig {queue}" if queue else ""
    cleanup = []
    results, plan, failure = [], None, None
    try:
        with fixture_local_dumps(machine, output, cleanup):
            desktop(machine, f"-File {GUEST_REPO}\\tools\\windows-vm\\run-fixtures.ps1 "
                             f"-Executable {directory}\\CapyCanvas.exe -Output {output} "
                             f"-TimeoutMinutes {args.timeout_minutes} -RustProfile {args.profile} "
                             f"-BuildRecord {directory}\\fixture-build.json{selection}{mode}")
            results, plan, failure = wait_for_fixtures(machine, output, drain)
    except (Exception, SystemExit) as error:
        failure = str(error)
    local = retrieve(machine, f"{GUEST_REPO}\\artifacts\\windows\\vm-fixtures", name)
    (local / "provenance.json").write_text(json.dumps(provenance, indent=2))
    if cleanup:
        (local / "wer").mkdir(exist_ok=True)
        (local / "wer" / "restore-error.txt").write_text("\n".join(cleanup))
        failure = "; ".join(filter(None, (failure, "WER restoration failed: " + "; ".join(cleanup))))
    failed = [result["name"] for result in results if result["exit"] != 0]
    print(f"{len(results) - len(failed)} passed, {len(failed)} failed. Logs: {local}")
    return {"vm": machine.name, "results": results, "plan": plan, "failure": failure,
            "complete": (local / "complete").exists() and not cleanup and failure is None, "evidence": str(local)}


def fixtures(args):
    if getattr(args, "keep_runners", False) and getattr(args, "runners", None) is None:
        raise ValueError("--keep-runners requires --runners")
    warp = getattr(args, "warp", False) or getattr(args, "runners", None) is not None
    hardware = getattr(args, "hardware_gpu", False)
    if hardware and warp:
        raise ValueError("CPU runners cannot be used with --hardware-gpu")
    if getattr(args, "workers", None) is not None and (hardware or warp):
        raise ValueError("--workers applies to shared GPU sessions; use --runners for WARP")
    if warp:
        return sharded_fixtures(args)
    if not hardware:
        return shared_fixtures(args)
    fixture_selection(args.names)
    machine = selected(args)
    start(argparse.Namespace(vm=args.vm, gui=False, gpu="auto"))
    with claim(machine):
        revision = run("git", "rev-parse", "HEAD", cwd=ROOT, capture_output=True, text=True).stdout.strip()
        hashes = sync_machine(machine)
        plan = planned_fixtures(machine, args, fixture_directory(True, args.profile))
        directory, record = fixture_build(machine, hashes, args.no_build, True, args.profile, controls=plan["controls"])
        result = execute_fixtures(machine, args, directory,
                                  {"revision": revision, "source": hashes, "build": record, "hardware_gpu": True}, args.names)
    if result["failure"] or not result["complete"]:
        sys.exit(result["failure"] or "The fixture runner stopped before finishing.")
    validate_results(result["plan"], result["results"])
    if any(row["exit"] != 0 for row in result["results"]):
        sys.exit(1)


def planned_fixtures(machine, args, directory):
    result = guest(machine, f"& '{GUEST_PWSH}' -NoProfile -File '{GUEST_REPO}\\tools\\windows-vm\\run-fixtures.ps1' "
                   f"-Executable '{directory}\\CapyCanvas.exe' -Output '{directory}' -List"
                   + fixture_selection(args.names), capture_output=True, text=True)
    plan = json.loads(result.stdout)
    selectors = plan["runs"]
    if not isinstance(selectors, list) or not selectors or len(set(selectors)) != len(selectors):
        raise ValueError("The guest returned an empty or duplicate fixture plan")
    fixture_selection(selectors)
    requirements = plan.get("requirements")
    if not isinstance(requirements, dict) or set(requirements) != set(selectors):
        raise ValueError("Fixture requirements do not cover the exact plan")
    for row in requirements.values():
        if not isinstance(row, dict) or type(row.get("hardware_gpu")) is not bool or not isinstance(row.get("controls"), list) or any(kind not in ("Feedback", "Popup", "LayerDrop") for kind in row["controls"]):
            raise ValueError("Invalid fixture adapter or control requirements")
    return plan


def fixture_durations():
    durations = {}
    roots = {ARTIFACTS, Path(SETTINGS.get("artifacts") or ARTIFACTS)}
    files = sorted({path for root in roots for path in root.glob("*/*/results.jsonl")},
                   key=lambda path: path.stat().st_mtime)
    for path in files:
        for line in path.read_text(encoding="utf-8-sig").splitlines():
            try:
                row = json.loads(line)
                if type(row["seconds"]) in (int, float) and math.isfinite(row["seconds"]) and row["seconds"] > 0:
                    durations[row["name"]] = row["seconds"]
            except (ValueError, KeyError):
                continue
    return durations


def fixture_roots(directory, record):
    root = directory.rsplit("\\", 1)[-1]
    return [root, *[root + "-" + kind for kind in record["controls"]]]


def export_fixture_build(machine, directory, record, local):
    archive = local / "build.tar.gz"
    roots = " ".join(fixture_roots(directory, record))
    with archive.open("wb") as stream:
        run(*ssh_command(machine, f"tar -czf - -C {GUEST_REPO}\\artifacts\\windows {roots}"), stdout=stream)
    return archive


def import_fixture_build(machine, directory, record, archive, hashes, profile):
    for name in fixture_roots(directory, record):
        target = f"{GUEST_REPO}\\artifacts\\windows\\{name}"
        guest(machine, f"if(Test-Path -LiteralPath '{target}'){{Remove-Item -LiteralPath '{target}' -Recurse -Force}}")
    build_record_path(machine, False, profile).unlink(missing_ok=True)
    guest(machine, f"New-Item -ItemType Directory -Force '{GUEST_REPO}\\artifacts\\windows'|Out-Null")
    with archive.open("rb") as stream:
        run(*ssh_command(machine, f"tar -xzmf - -C {GUEST_REPO}\\artifacts\\windows"), stdin=stream)
    validate_build(record, build_inputs(hashes), build_files(machine, directory), profile)
    validate_controls(machine, directory, record, False, record["controls"])
    publish_build_record(machine, directory, record)
    build_record_path(machine, False, profile).write_text(json.dumps(record))


@contextmanager
def fixture_queue_config(machine, queue):
    path = f"{GUEST_REPO}\\artifacts\\windows\\fixture-queue-{secrets.token_hex(16)}.json"
    try:
        payload = json.dumps(queue.configuration(machine.name))
        guest(machine, f"[IO.File]::WriteAllText('{path}', [Console]::In.ReadToEnd())",
              input=payload, text=True, stdout=subprocess.DEVNULL)
        yield path
    finally:
        guest(machine, f"if(Test-Path -LiteralPath '{path}'){{Remove-Item -LiteralPath '{path}' -Force -ErrorAction Stop}}")


def validate_round_display(record):
    display = record.get("display", {})
    if record.get("restart_required") or not record.get("success") or not display.get("primary") or "NVIDIA" not in display.get("adapter", "") or display.get("refreshHz") != 120 or (
            display.get("width"), display.get("height"), display.get("scalePercent")) != (2560, 1600, 150):
        raise ValueError("Round GPU requires a verified 2560x1600 desktop at 150% scale")


def fixture_memory_gib(memory):
    size = re.fullmatch(r"([1-9][0-9]*)([KMGT]?)", str(memory), re.IGNORECASE)
    if not size:
        raise ValueError("Fixture VM memory must be a positive integer with an optional K, M, G or T suffix")
    power = {"K": 10, "M": 20, "": 20, "G": 30, "T": 40}[size[2].upper()]
    return ((int(size[1]) << power) + (1 << 30) - 1) // (1 << 30)


def fixture_memory_admission(local, phase, sizes):
    available = next(int(line.split()[1]) * 1024 for line in Path("/proc/meminfo").read_text().splitlines()
                     if line.startswith("MemAvailable:"))
    required = (sum(sizes) + 40) * 1024 ** 3
    receipt = {"phase": phase, "available_bytes": available, "required_bytes": required,
               "vm_sizes_gib": sizes, "admitted": available >= required}
    with (local / "memory-admission.jsonl").open("a") as output:
        output.write(json.dumps(receipt) + "\n")
    if not receipt["admitted"]:
        raise RuntimeError(f"{phase}: MemAvailable {available} bytes is below required {required}")


def sharded_fixtures(args):
    if args.hardware_gpu:
        raise ValueError("CPU runners cannot be used with --hardware-gpu")
    fixture_selection(args.names)
    primary = selected(args)
    if primary.runner.exists():
        raise ValueError("Build and shard from the worktree's primary VM")
    local = ARTIFACTS / primary.name / ("shards-" + time.strftime("%Y%m%d-%H%M%S"))
    local.mkdir(parents=True, exist_ok=False)
    started = time.monotonic()
    used, locks, shards = [], {}, []
    selectors, cpu_selectors, gpu_selectors = [], [], []
    provenance = {}
    primary_owned = False
    warm_cpu = False
    def exact_names(names):
        return [name if ":" in name else name + ":default" for name in names]
    def start_runner(machine):
        lock = claim(machine)
        try:
            boot(machine, gui=False)
            return machine, lock, None
        except (Exception, SystemExit) as error:
            try:
                if getattr(machine, "launched_pid", None) is not None and machine.qemu_pid.exists() and int(machine.qemu_pid.read_text()) == machine.launched_pid:
                    stop_machine(machine)
            finally:
                lock.close()
            if isinstance(error, cpu_pool.Unavailable):
                return None, None, str(error)
            raise
    def run_cpu(machine):
        result = {"vm": machine.name, "results": [], "complete": False, "failure": None}
        try:
            directory, record = builds[False]
            if machine is not primary:
                synced = sync_machine(machine)
                if synced != hashes:
                    raise ValueError("The source tree changed while preparing fixture runners")
                import_fixture_build(machine, directory, record, archive, synced, args.profile)
            with (fixture_queue_config(machine, queue) if queue else nullcontext(None)) as configuration:
                result = execute_fixtures(machine, args, directory, mode_provenance(False), exact_names(cpu_selectors),
                                          queue=configuration, drain=(lambda: queue.drain(machine.name)) if queue else None)
        except (Exception, SystemExit) as error:
            result["failure"] = "; ".join(filter(None, (result.get("failure"), str(error))))
        return dict(result, selectors=list(queue.assignments[machine.name]) if queue else cpu_selectors)

    def mode_provenance(hardware):
        return {"revision": provenance["revision"], "source": hashes,
                "build": builds[hardware][1], "hardware_gpu": hardware}
    def run_gpu():
        nonlocal primary_owned
        result = {"vm": primary.name, "results": [], "complete": False, "failure": None}
        try:
            with claim(primary):
                try:
                    if sync_machine(primary) != hashes:
                        raise ValueError("The source tree changed while preparing GPU fixtures")
                    directory, expected = builds[True]
                    directory, record = fixture_build(primary, hashes, True, True, args.profile,
                                                       controls=list(expected["controls"]))
                    if record != expected:
                        raise ValueError("The retained hardware build changed before GPU execution")
                    publish_build_record(primary, directory, dict(record, fixture_scale=1.5))
                    result = execute_fixtures(primary, args, directory, mode_provenance(True), exact_names(gpu_selectors))
                finally:
                    try:
                        stop_machine(primary)
                        primary_owned = False
                    except (Exception, SystemExit) as error:
                        result["failure"] = "; ".join(filter(None, (result.get("failure"), f"GPU shutdown: {error}")))
        except (Exception, SystemExit) as error:
            result["failure"] = "; ".join(filter(None, (result.get("failure"), str(error))))
        return dict(result, selectors=gpu_selectors)
    try:
        with claim(primary):
            if primary.gpu.exists() and running(primary):
                raise ValueError("Stop the primary GPU VM before preparing a fixture round")
            if not running(primary):
                fixture_memory_admission(local, "build-primary", [fixture_memory_gib(MEMORY)])
            primary_owned = True
            boot(primary, gui=False)
            hashes = sync_machine(primary)
            plan = dict(planned_fixtures(primary, args, fixture_directory(False, args.profile)))
            selectors = plan["runs"]
            requirements = plan["requirements"]
            cpu_selectors = [name for name in selectors if not requirements[name]["hardware_gpu"]]
            gpu_selectors = [name for name in selectors if requirements[name]["hardware_gpu"]]
            warm_cpu = args.runners is None and bool(args.names) and not gpu_selectors
            primary_owned = not warm_cpu
            (local / "plan.json").write_text(json.dumps(plan, indent=2))
            if gpu_selectors and (str(CPUS), MEMORY, DISPLAY) != ("8", "16G", "2560x1600"):
                raise ValueError("Round GPU requires existing 8-vCPU / 16-GiB, 2560x1600 settings")
            builds = {}
            for hardware, names in ((False, cpu_selectors), (True, gpu_selectors)):
                if names:
                    controls = sorted({kind for name in names for kind in requirements[name]["controls"]})
                    builds[hardware] = fixture_build(primary, hashes, args.no_build, hardware, args.profile, controls=controls)
            provenance = {"revision": run("git", "rev-parse", "HEAD", cwd=ROOT, capture_output=True, text=True).stdout.strip(),
                          "source": hashes, "build": builds[False if cpu_selectors else True][1], "hardware_gpu": not bool(cpu_selectors),
                          "builds": {"hardware" if mode else "software": record for mode, (_, record) in builds.items()}}
            (local / "provenance.json").write_text(json.dumps(provenance, indent=2))
            if warm_cpu:
                queue = None
                shards = [run_cpu(primary)]
            else:
                archive = export_fixture_build(primary, *builds[False], local) if cpu_selectors else None
                stop_machine(primary)
                primary_owned = False
        if gpu_selectors:
            fixture_memory_admission(local, "gpu-primary", [16])
            primary_owned = True
            start(argparse.Namespace(vm=primary.name, gui=False, gpu="auto", round_gpu=True))
            display = json.loads((primary.directory / "display.json").read_text())
            (local / "gpu-display.json").write_text(json.dumps(display, indent=2))
            shutil.copyfile(primary.directory / "fixture-scale-change.json", local / "gpu-scale-change.json")
            validate_round_display(display)
        if cpu_selectors and not warm_cpu:
            settings, slots = cpu_pool.describe(STATE)
            free = sum(row["status"] == "free" for row in slots)
            owned = sum(bool(row["owner"]) and row["owner"].get("worktree") == str(ROOT.resolve()) for row in slots)
            limit = min(args.runners or 1, len(cpu_selectors), settings["per_owner_limit"] - (2 if gpu_selectors else 0))
            candidates = [runner_machine(primary, index + 1) for index in range(limit)]
            retained = [machine for machine in candidates if running(machine)]
            new = [machine for machine in candidates if machine not in retained]
            count = min(free, max(0, settings["per_owner_limit"] - owned))
            candidates = retained + new[:count]
            if not candidates:
                raise RuntimeError("No CPU runner slot was available; see `runners`")
            fixture_memory_admission(local, "cpu-batch", [settings["runner_memory_gib"]] * min(count, len(new)))
            count = len(candidates)
            with ThreadPoolExecutor(max_workers=count) as executor:
                futures = [executor.submit(start_runner, machine) for machine in candidates]
                failures = []
                for future in futures:
                    try:
                        machine, lock, error = future.result()
                        if machine:
                            used.append(machine)
                            locks[machine.name] = lock
                        else:
                            print(error, flush=True)
                    except (Exception, SystemExit) as error:
                        failures.append(str(error))
                if failures:
                    raise RuntimeError("; ".join(failures))
            if not used:
                raise RuntimeError("No CPU runner slot was available; see `runners`")
        if not warm_cpu:
            with (fixture_shards.Queue(cpu_selectors, [machine.name for machine in used], fixture_durations(), local / "cpu-plan.json")
                  if used else nullcontext(None)) as queue:
                with ThreadPoolExecutor(max_workers=len(used) + bool(gpu_selectors)) as executor:
                    futures = [executor.submit(run_cpu, machine) for machine in used]
                    if gpu_selectors:
                        futures.append(executor.submit(run_gpu))
                    shards = [future.result() for future in futures]
    except (Exception, SystemExit) as error:
        shards.append({"vm": primary.name + "-setup", "selectors": [], "results": [],
                       "complete": False, "failure": str(error)})
    finally:
        cleanup_errors = []
        for machine in [*used, *([primary] if primary_owned else [])]:
            try:
                if machine is primary or not args.keep_runners:
                    stop_machine(machine)
            except (Exception, SystemExit) as error:
                cleanup_errors.append(f"{machine.name}: {error}")
            finally:
                if machine.name in locks:
                    locks[machine.name].close()
        if selectors:
            results, errors = fixture_shards.merge(selectors, shards)
        else:
            results, errors = [], ["No fixture plan was prepared", *[row["failure"] for row in shards if row.get("failure")]]
        errors.extend(cleanup_errors)
        if selectors:
            plan["shards"] = {row["vm"]: row["selectors"] for row in shards}
            (local / "plan.json").write_text(json.dumps(plan, indent=2))
        (local / "results.jsonl").write_text("".join(json.dumps(row) + "\n" for row in results))
        (local / "shards.json").write_text(json.dumps(shards, indent=2))
        (local / "summary.json").write_text(json.dumps({"wall_seconds": time.monotonic() - started, "errors": errors}, indent=2))
        if cleanup_errors:
            (local / "shutdown-errors.json").write_text(json.dumps(cleanup_errors, indent=2))
    destination = "the primary VM" if warm_cpu else f"{len(used)} CPU runners and {int(bool(gpu_selectors))} GPU VM"
    print(f"{sum(row['exit'] == 0 for row in results)}/{len(selectors)} passed on {destination}. Logs: {local}")
    if errors:
        raise RuntimeError("; ".join(errors))
    (local / "complete").touch()


def shared_settings():
    return {"vms": 2, "sessions": 15, "cpu_slots": 8, "idle_minutes": 10} | SETTINGS.get("shared", {})


def shared_accounts():
    return [f"capy{index:02}" for index in range(1, shared_settings()["sessions"] + 1)]


def shared_machines():
    names = [f"{SHARED_PREFIX}{index}" for index in range(1, shared_settings()["vms"] + 1)]
    return [Machine(name, VMS / name) for name in names]


def shared_pool():
    return session_pool.Pool(RUNTIME / "shared-sessions")


def shared_unit(machine):
    return f"capy-shared-{hashlib.sha256(bytes(STATE)).hexdigest()[:8]}-{machine.name}"


def host_alive(machine):
    machine.runtime.mkdir(mode=0o700, parents=True, exist_ok=True)
    lock = cpu_pool.lock_file(machine.runtime / "host-lock")
    if lock is None:
        return True
    lock.close()
    return False


def write_session_health(machine, name, value):
    directory = machine.runtime / "session-health"
    directory.mkdir(mode=0o700, parents=True, exist_ok=True)
    path = directory / (name + ".json")
    temporary = path.with_suffix(f".{os.getpid()}.tmp")
    temporary.write_text(json.dumps(value))
    temporary.replace(path)


def initialize_session_health(machine):
    result = guest(machine, "(Get-CimInstance Win32_OperatingSystem).LastBootUpTime.ToUniversalTime().ToString('o')",
                   stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=60)
    boot = result.stdout.strip()
    if not boot:
        raise RuntimeError("The shared guest boot identity is unavailable")
    directory = machine.runtime / "session-health"
    try:
        previous = json.loads((directory / "boot.json").read_text())
    except (OSError, ValueError):
        previous = None
    if previous != boot:
        for account in shared_accounts():
            (directory / (account + ".json")).unlink(missing_ok=True)
    write_session_health(machine, "boot", boot)


def withdrawn_accounts(machine):
    directory = machine.runtime / "session-health"
    return {account for account in shared_accounts() if (directory / (account + ".json")).exists()}


def session_evidence_pending(machine):
    for account in withdrawn_accounts(machine):
        try:
            value = json.loads((machine.runtime / "session-health" / (account + ".json")).read_text())
            if value.get("evidence_state") == "pending" and value["evidence_deadline"] > time.time():
                return True
        except (OSError, ValueError, KeyError, TypeError):
            continue
    return False


def ready_accounts(machine):
    if not host_alive(machine):
        return []
    try:
        return [account for account in json.loads((machine.runtime / "sessions.json").read_text())
                if account not in withdrawn_accounts(machine)]
    except (OSError, ValueError):
        return []


def start_shared_vm(machine):
    if host_alive(machine):
        return
    machine.runtime.mkdir(mode=0o700, parents=True, exist_ok=True)
    (machine.runtime / "startup-error").unlink(missing_ok=True)
    program = [sys.executable, str(Path(__file__).resolve()), "--vm", machine.name, "shared-host"]
    environment = {"CAPYCANVAS_VM_DIR": str(STATE), "CAPYCANVAS_VM_RUNTIME": str(RUNTIME),
                   "CAPYCANVAS_VM_DISPLAY": SHARED_DISPLAY}
    unit = shared_unit(machine)
    run("systemctl", "--user", "reset-failed", unit, check=False, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    run("systemd-run", "--user", "--quiet", f"--unit={unit}", "--collect", "--property=KillMode=process",
        "--property=LimitMEMLOCK=infinity", "--property=TimeoutStopSec=6min",
        *[f"--setenv={key}={value}" for key, value in environment.items()],
        *[item.replace("$", "$$") for item in program], check=False, stderr=subprocess.DEVNULL)


class RdpClients:
    def __init__(self, machine):
        self.machine = machine
        self.directory = machine.runtime / "rdp"
        self.record = self.directory / "clients.json"
        self.children = {}
        self.directory.mkdir(mode=0o700, parents=True, exist_ok=True)
        self.stop_recorded()

    def stop_recorded(self):
        if not self.record.exists():
            return
        for row in json.loads(self.record.read_text()):
            command = Path("/proc") / str(row["pid"]) / "cmdline"
            if command.exists() and row["identity"].encode() in command.read_bytes():
                os.kill(row["pid"], signal.SIGTERM)
        self.record.unlink()

    def save(self):
        rows = [{"pid": child.pid, "identity": str(child.args[0])}
                for pair in self.children.values() for child in pair]
        self.record.write_text(json.dumps(rows))

    def start(self, account):
        client, xvfb = rdp_tools()
        if not client or not xvfb:
            raise RuntimeError(f"FreeRDP's X11 client or Xvfb is missing; run `{PROGRAM} setup --server`.")
        number = self.directory / f"{account}-display"
        with number.open("w") as output, (self.directory / f"{account}-xvfb.log").open("w") as log:
            display = subprocess.Popen([str(xvfb), "-displayfd", str(output.fileno()), "-screen", "0", f"{SHARED_DISPLAY}x24",
                                        "-nolisten", "tcp", "-noreset", "-ac"], stdout=log, stderr=log,
                                       pass_fds=(output.fileno(),), start_new_session=True)
        self.children[account] = [display]
        self.save()
        deadline = time.monotonic() + 15
        while not number.read_text().strip():
            if display.poll() is not None or time.monotonic() > deadline:
                raise RuntimeError(f"The private display of {account} did not start.")
            time.sleep(0.1)
        environment = dict(os.environ, DISPLAY=":" + number.read_text().strip(), XDG_CONFIG_HOME=str(self.directory),
                           XDG_CACHE_HOME=str(self.directory), XDG_STATE_HOME=str(self.directory))
        if str(client).startswith(str(RDP_CLIENT)):
            environment["LD_LIBRARY_PATH"] = str(RDP_CLIENT / "lib64")
        width, height = SHARED_DISPLAY.split("x")
        with (self.directory / f"{account}.log").open("w") as log:
            session = subprocess.Popen([str(client), f"/v:127.0.0.1:{self.machine.rdp_port.read_text().strip()}",
                                        f"/u:{account}", "/d:capycanvas-vm", "/from-stdin:force", "/cert:ignore",
                                        f"/size:{width}x{height}", "/scale-desktop:150", "/scale-device:100", "/network:lan",
                                        "/gdi:sw", "-clipboard", "-wallpaper", "-suppress-output", "/log-level:WARN"],
                                       stdin=subprocess.PIPE, stdout=log, stderr=log, env=environment, start_new_session=True)
        session.stdin.write(SERVER_PASSWORD.read_bytes())
        session.stdin.close()
        self.children[account].append(session)
        self.save()

    def keep_connected(self, accounts):
        for account in accounts:
            pair = self.children.get(account)
            if pair and all(child.poll() is None for child in pair):
                continue
            for child in pair or ():
                self.stop_child(child)
            self.start(account)

    def stop_child(self, child):
        if child.poll() is None:
            child.terminate()
            try:
                child.wait(timeout=10)
            except subprocess.TimeoutExpired:
                child.kill()
                child.wait()

    def close(self):
        for pair in self.children.values():
            for child in pair:
                self.stop_child(child)
        self.children.clear()
        self.record.unlink(missing_ok=True)


SESSION_SHELLS_COMMAND = "$shells=@(Get-Process explorer -ErrorAction SilentlyContinue);" \
    "@{shells=@($shells|ForEach-Object {$_.SessionId});sessions=@(quser.exe 2>$null)}|ConvertTo-Json -Compress"


def session_shells(machine):
    result = guest(machine, SESSION_SHELLS_COMMAND,
                   stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=60)
    listing = json.loads(result.stdout)
    shells = set(listing["shells"])
    accounts = set()
    for line in listing["sessions"]:
        match = re.match(r"\s*>?(\S+)\s+(?:\S+\s+)?(\d+)\s+", line)
        if match and int(match[2]) in shells:
            accounts.add(match[1].rsplit("\\", 1)[-1].lower())
    return accounts


def shared_host(args):
    machine = selected(args)
    if not machine.server:
        raise ValueError("Only shared GPU VMs have a session host")
    machine.runtime.mkdir(mode=0o700, parents=True, exist_ok=True)
    lock = cpu_pool.lock_file(machine.runtime / "host-lock")
    if lock is None:
        return
    stopping = []
    def terminated(*_):
        stopping.append(True)
        sys.exit(0)
    signal.signal(signal.SIGTERM, terminated)
    record = machine.runtime / "sessions.json"
    record.unlink(missing_ok=True)
    settings = shared_settings()
    accounts = shared_accounts()
    pool = shared_pool()
    clients = RdpClients(machine)
    try:
        fresh = not running(machine)
        if fresh:
            fixture_memory_admission(machine.runtime, "shared-vm", [settings["cpu_slots"] * cpu_pool.configuration(STATE)["runner_memory_gib"]])
        boot(machine, gui=False, gpu="auto")
        initialize_session_health(machine)
        if fresh:
            guest(machine, f"Get-ChildItem {SHARED_ROOT} -ErrorAction SilentlyContinue|Remove-Item -Recurse -Force -ErrorAction SilentlyContinue",
                  stdin=subprocess.DEVNULL)
        idle_since = time.monotonic()
        while running(machine):
            clients.keep_connected(accounts)
            polled = time.monotonic()
            try:
                shells = session_shells(machine)
            except subprocess.TimeoutExpired as error:
                elapsed = time.monotonic() - polled
                try:
                    guest(machine, "exit 0", stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=20)
                except (subprocess.TimeoutExpired, subprocess.CalledProcessError):
                    (machine.runtime / "startup-error").write_text(
                        f"{machine.name} stopped responding to SSH health checks (timeout {error.timeout} s)")
                    break
                with (machine.runtime / "slow-health.jsonl").open("a") as output:
                    output.write(json.dumps({"utc": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
                                             "command": SESSION_SHELLS_COMMAND, "elapsed": elapsed}) + "\n")
                shells = None
            if shells is not None:
                temporary = record.with_suffix(".tmp")
                temporary.write_text(json.dumps([account for account in accounts if account in shells and account not in withdrawn_accounts(machine)]))
                temporary.replace(record)
            if pool.holders(machine.name, accounts) or session_evidence_pending(machine):
                idle_since = time.monotonic()
            elif time.monotonic() - idle_since >= settings["idle_minutes"] * 60:
                with pool.closed(machine.name, accounts) as closed:
                    if closed:
                        record.unlink()
                        break
            time.sleep(15)
    except (Exception, SystemExit) as error:
        if not stopping:
            (machine.runtime / "startup-error").write_text(str(error))
        raise
    finally:
        record.unlink(missing_ok=True)
        clients.close()
        stop_machine(machine)


class SharedSession:
    def __init__(self, machine, account, lock, run_id, lease):
        self.machine = machine
        self.account = account
        self.lock = lock
        self.slot = f"{machine.name}-{account}"
        self.id = f"{self.slot}-{lease}"
        self.root = f"{SHARED_ROOT}\\{run_id}\\{lease}"
        self.repo = self.root + "\\repo"
        self.task = f"capycanvas-{account}"
        self.started = None
        self.launched = False
        self.closing = None
        self.closed = False
        self.draining = False
        self.complete = False
        self.failure = None
        self.results = []
        self.progress = time.monotonic()
        self.evidence = None
        self.graphics_withdrawal = None
        self.graphics_identity = None
        self.last_passed = None


def session_status_command(sessions):
    rows = ",".join(f"@{{id='{session.id}';output='{session.root}\\output';task='{session.task}'}}" for session in sessions)
    return r"""$ErrorActionPreference='Stop'
$ProgressPreference='SilentlyContinue'
[Console]::OutputEncoding=[Text.UTF8Encoding]::new($false)
function Read-FixtureText([string]$Path){
    if(!(Test-Path -LiteralPath $Path)){return $null}
    $stream=[IO.File]::Open($Path,[IO.FileMode]::Open,[IO.FileAccess]::Read,([IO.FileShare]::ReadWrite -bor [IO.FileShare]::Delete))
    $reader=[IO.StreamReader]::new($stream)
    try{$reader.ReadToEnd()}finally{$reader.Dispose();$stream.Dispose()}
}
$rows=@{}
foreach($row in @(""" + rows + r""")){
    $state=[string](Get-ScheduledTask $row.task).State
    $complete=Test-Path "$($row.output)\complete"
    $rows[$row.id]=@{state=$state;complete=$complete;log=Read-FixtureText "$($row.output)\results.jsonl";error=Read-FixtureText "$($row.output)\runner-error.txt";graphics_withdrawal=Read-FixtureText "$($row.output)\graphics-withdrawal.json";graphics_identity=Read-FixtureText "$($row.output)\graphics-identity.json"}
}
ConvertTo-Json -InputObject $rows -Depth 4 -Compress
"""


class SharedRun:
    def __init__(self, args, local, identity, queue, update_demand, payload):
        self.args = args
        self.local = local
        self.id = identity
        self.queue = queue
        self.update_demand = update_demand
        self.payload = payload
        self.pool = shared_pool()
        self.machines = shared_machines()
        self.sessions = []
        self.prepared = {}
        self.locks = {machine.name: Lock() for machine in self.machines}
        self.broken = set()
        self.errors = []
        self.start_attempts = {}
        self.polled = 0
        self.poll_failures = {}
        self.peak_sessions = 0

    def payload_root(self):
        return f"{SHARED_ROOT}\\{self.id}\\payload"

    def prepare_machine(self, machine):
        with self.locks[machine.name]:
            if self.prepared.get(machine.name):
                return
            root = self.payload_root()
            directory = fixture_directory(True, self.args.profile).replace(GUEST_REPO, root)
            live = ",".join(f"'{run}'" for run in self.pool.runs())
            guest_script(machine, f"""$ProgressPreference='SilentlyContinue'
Get-ChildItem {SHARED_ROOT} -ErrorAction SilentlyContinue|Where-Object {{$_.Name -notin @({live}) -and $_.CreationTime -lt (Get-Date).AddMinutes(-5)}}|Remove-Item -Recurse -Force -ErrorAction SilentlyContinue
$ErrorActionPreference='Stop'
New-Item -ItemType Directory -Force '{root}\\artifacts\\windows'|Out-Null
""", timeout=300)
            for archive, destination in ((self.payload["source"], root), (self.payload["build"], root + "\\artifacts\\windows")):
                with archive.open("rb") as stream:
                    run(*ssh_command(machine, f"tar -xzmf - -C {destination}"), stdin=stream)
            record = self.payload["record"]
            validate_build(record, build_inputs(self.payload["hashes"]), build_files(machine, directory), self.args.profile)
            validate_controls(machine, directory, record, True, record["controls"])
            publish_build_record(machine, directory, dict(record, fixture_scale=1.5))
            run_root = root.rsplit("\\", 1)[0]
            guest_script(machine, (GUEST_FILES / "session-view.ps1").read_text() + f"""
$ErrorActionPreference='Stop'
Protect-FixtureSessionPayload -Path '{run_root}' -User '{USER}'
""")
            self.prepared[machine.name] = directory

    def start_session(self, session):
        machine = session.machine
        self.prepare_machine(machine)
        directory = self.prepared[machine.name].replace(self.payload_root(), session.repo)
        queue = json.dumps(self.queue.configuration(session.id))
        guest_script(machine, (GUEST_FILES / "session-view.ps1").read_text() + f"""
$ErrorActionPreference='Stop'
$ProgressPreference='SilentlyContinue'
$owned=@(Get-Process -IncludeUserName -ErrorAction SilentlyContinue|Where-Object {{$_.UserName -like '*\\{session.account}' -and $_.ProcessName -in 'pwsh','powershell','conhost','CapyCanvas','capy-photo-reader'}})
$owned|Stop-Process -Force -ErrorAction SilentlyContinue
Remove-Item -LiteralPath 'C:\\Users\\{session.account}\\AppData\\Local\\CrashDumps' -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force '{session.root}'|Out-Null
& icacls.exe '{session.root}' /grant '{session.account}:(OI)(CI)F' /Q|Out-Null
if($LASTEXITCODE){{throw 'Could not grant the session its workspace'}}
New-FixtureSessionView -Payload '{self.payload_root()}' -Repository '{session.repo}' -BuildDirectory '{self.prepared[machine.name]}'
[IO.File]::WriteAllText('{session.root}\\queue.json','{queue}',[Text.UTF8Encoding]::new($false))
$global:LASTEXITCODE=0
""", timeout=900)
        desktop(machine, f"-File {session.repo}\\tools\\windows-vm\\session-worker.ps1 -Executable {directory}\\CapyCanvas.exe "
                         f"-Output {session.root}\\output -BuildRecord {directory}\\fixture-build.json "
                         f"-QueueConfig {session.root}\\queue.json -RustProfile {self.args.profile} "
                         f"-TimeoutMinutes {self.args.timeout_minutes}",
                user=session.account, task=session.task, directory=session.repo)
        session.launched = True
        session.progress = time.monotonic()

    def close_session(self, session):
        machine = session.machine
        try:
            if running(machine):
                guest_script(machine, f"""$ProgressPreference='SilentlyContinue'
Stop-ScheduledTask {session.task} -ErrorAction SilentlyContinue
Unregister-ScheduledTask {session.task} -Confirm:$false -ErrorAction SilentlyContinue
$dumps='C:\\Users\\{session.account}\\AppData\\Local\\CrashDumps'
if((Test-Path -LiteralPath '{session.root}\\output') -and (Test-Path -LiteralPath $dumps)){{New-Item -ItemType Directory -Force '{session.root}\\output\\wer\\dumps'|Out-Null;Copy-Item -Path "$dumps\\*" -Destination '{session.root}\\output\\wer\\dumps' -Force}}
""", timeout=300)
                if session.launched:
                    session.evidence = str(retrieve(machine, session.root, "output", self.local / "sessions" / session.id))
                guest(machine, f"Remove-Item -LiteralPath '{session.root}' -Recurse -Force -ErrorAction SilentlyContinue",
                      stdin=subprocess.DEVNULL, timeout=600, check=False)
        except (Exception, SystemExit) as error:
            session.complete = False
            session.failure = "; ".join(filter(None, (session.failure, f"evidence: {error}")))
        finally:
            session.lock.close()
            session.closed = True

    def fail_session(self, session, failure):
        self.broken.add(session.slot)
        session.failure = "; ".join(filter(None, (session.failure, failure)))
        session.draining = True
        self.queue.drain(session.id)
        print(f"{session.id}: {failure}", flush=True)

    def poll(self, executor):
        groups = {}
        for session in self.sessions:
            if session.started and session.started.done() and not session.started.exception() and not session.closing:
                groups.setdefault(session.machine.name, (session.machine, []))[1].append(session)
        if not groups:
            return
        completed = []
        with ThreadPoolExecutor(max_workers=len(groups)) as queries:
            pending = [(machine, group, queries.submit(guest, machine, session_status_command(group),
                        stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=120))
                       for machine, group in groups.values()]
            for machine, group, future in pending:
                try:
                    result = future.result()
                    statuses = json.loads(result.stdout)
                    self.poll_failures[machine.name] = 0
                except (subprocess.SubprocessError, ValueError, OSError) as error:
                    self.poll_failures[machine.name] = self.poll_failures.get(machine.name, 0) + 1
                    if self.poll_failures[machine.name] >= 30 or not running(machine):
                        for session in group:
                            self.fail_session(session, f"lost contact with {machine.name}: {error}")
                            session.closing = executor.submit(self.close_session, session)
                    continue
                for session in group:
                    status = statuses[session.id]
                    rows = (status["log"] or "").splitlines()
                    for line in rows[len(session.results):]:
                        try:
                            result = json.loads(line)
                        except ValueError:
                            break
                        session.results.append(result)
                        session.progress = time.monotonic()
                        if result["exit"] == 0:
                            session.last_passed = dict(result, observed_utc=time.time())
                        verdict = "infrastructure-invalid" if result.get("infrastructure_invalid") else "pass" if result["exit"] == 0 else "FAIL"
                        print(f"{verdict} {result['seconds']:5.0f}s {result['name']} [{session.id}]", flush=True)
                    identity = status.get("graphics_identity")
                    if identity:
                        session.graphics_identity = json.loads(identity)
                completed.append((group, statuses))
        withdrawals = []
        for group, statuses in completed:
            for session in group:
                status = statuses[session.id]
                withdrawal = json.loads(status["graphics_withdrawal"]) if status.get("graphics_withdrawal") else None
                if not withdrawal and session.results and session.results[-1].get("infrastructure_invalid") and session.graphics_identity:
                    row = session.results[-1]
                    withdrawal = dict(session.graphics_identity, name=row["name"], exit=row["exit"],
                                      error=row["message"], cause=row["infrastructure_cause"])
                if withdrawal and session.graphics_withdrawal is None:
                    withdrawals.append((session, withdrawal,
                                        [peer for peer in group if not statuses[peer.id].get("graphics_withdrawal")]))
        if withdrawals:
            with ThreadPoolExecutor(max_workers=len(withdrawals)) as captures:
                pending = [captures.submit(self.withdraw_graphics_session, *row) for row in withdrawals]
                for future in pending:
                    future.result()
        for group, statuses in completed:
            for session in group:
                status = statuses[session.id]
                if status["complete"]:
                    session.complete = True
                elif status["state"] not in ("Running", "Queued"):
                    self.fail_session(session, status["error"] or "The session worker stopped before completing its fixtures.")
                elif time.monotonic() - session.progress > self.args.timeout_minutes * 60 + 300:
                    self.fail_session(session, "The session worker exceeded its fixture deadline.")
                else:
                    continue
                session.closing = executor.submit(self.close_session, session)

    def capture_session_graphics(self, session, identity, target, role):
        output = f"{session.root}\\output\\graphics-{target.id}-{role}.json"
        task = f"capygraphics-{self.id}-{session.account}-{secrets.token_hex(4)}"
        receipt = {"account": session.account, "identity": identity, "task": task,
                   "role": role, "output": output, "limit_seconds": 180, "started": time.time()}
        if role == "healthy":
            receipt["passed_row"] = session.last_passed
            receipt["passed_row_evidence"] = str(self.local / "sessions" / session.id / "output" / "results.jsonl")
        directory = self.local / "graphics" / target.id
        directory.mkdir(parents=True, exist_ok=True)
        deadline = time.monotonic() + 180
        try:
            arguments = (f"-File {session.repo}\\tools\\windows-vm\\session-graphics-evidence.ps1 "
                         f'-Output {output} -ExpectedAccount "{identity["account"]}" '
                         f'-ExpectedSessionId {int(identity["session_id"])} -ExpectedBootUtc "{identity["boot_utc"]}" '
                         f'-ExpectedLogonSid "{identity["logon_sid"]}"')
            if identity.get("user_sid"):
                arguments += f' -ExpectedUserSid "{identity["user_sid"]}"'
            desktop(session.machine, arguments, user=session.account, task=task, directory=session.repo)
            while time.monotonic() < deadline:
                result = guest(session.machine, f"$ProgressPreference='SilentlyContinue';"
                               f"if(Test-Path -LiteralPath '{output}'){{Get-Content -LiteralPath '{output}' -Raw -Encoding UTF8}}",
                               stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=20)
                if result.stdout.strip():
                    captured = json.loads(result.stdout)
                    (directory / (role + ".json")).write_text(json.dumps(captured, indent=2))
                    if captured.get("status") != "collected" or not captured.get("same_logon_verified"):
                        raise RuntimeError("The graphics evidence did not verify the requested private logon; see its retained JSON")
                    receipt["status"] = "retrieved"
                    break
                time.sleep(2)
            else:
                raise RuntimeError("The in-session graphics evidence exceeded its 180 s capture window")
        except (Exception, SystemExit) as error:
            receipt.update(status="unavailable", error=str(error))
        finally:
            receipt["finished"] = time.time()
            (directory / (role + "-receipt.json")).write_text(json.dumps(receipt, indent=2))
            try:
                guest(session.machine, f"Stop-ScheduledTask -TaskName '{task}' -ErrorAction SilentlyContinue;"
                      f"Unregister-ScheduledTask -TaskName '{task}' -Confirm:$false -ErrorAction SilentlyContinue",
                      stdin=subprocess.DEVNULL, timeout=30, check=False)
            except (Exception, SystemExit) as error:
                receipt["task_cleanup_error"] = str(error)
                (directory / (role + "-receipt.json")).write_text(json.dumps(receipt, indent=2))
        return receipt

    def withdraw_graphics_session(self, session, withdrawal, peers):
        account = withdrawal["account"].rsplit("\\", 1)[-1].lower()
        if account != session.account or not withdrawal.get("session_id") or not withdrawal.get("logon_sid"):
            raise RuntimeError("The graphics withdrawal has no matching private-session identity")
        boot = json.loads((session.machine.runtime / "session-health" / "boot.json").read_text())
        if withdrawal["boot_utc"] != boot:
            raise RuntimeError("The graphics withdrawal belongs to a different guest boot")
        session.graphics_withdrawal = withdrawal
        session.draining = True
        self.queue.drain(session.id)
        record = dict(withdrawal, run=self.id, worker=session.id, evidence_state="pending",
                      evidence_started=time.time(), evidence_deadline=time.time() + 360)
        write_session_health(session.machine, session.account, record)
        healthy = next((peer for peer in peers if peer is not session and not peer.closing and not peer.draining
                        and peer.last_passed and time.time() - peer.last_passed["observed_utc"] <= 300
                        and peer.results and peer.results[-1]["exit"] == 0 and peer.graphics_identity
                        and peer.graphics_identity["boot_utc"] == boot
                        and peer.account not in withdrawn_accounts(session.machine)), None)
        try:
            with ThreadPoolExecutor(max_workers=2) as captures:
                pending = [captures.submit(self.capture_session_graphics, session, withdrawal, session, "affected")]
                if healthy:
                    pending.append(captures.submit(self.capture_session_graphics, healthy, healthy.graphics_identity, session, "healthy"))
                else:
                    directory = self.local / "graphics" / session.id
                    directory.mkdir(parents=True, exist_ok=True)
                    (directory / "healthy-receipt.json").write_text(json.dumps({
                        "status": "unavailable", "reason": "No same-VM leased account has a passing row and a verified worker identity"}, indent=2))
                record["captures"] = [future.result() for future in pending]
        finally:
            record.update(evidence_state="finished", evidence_finished=time.time())
            try:
                guest(session.machine, f"[IO.File]::WriteAllText('{session.root}\\output\\graphics-evidence-complete',"
                      f"'{record['evidence_finished']}',[Text.UTF8Encoding]::new($false))",
                      stdin=subprocess.DEVNULL, timeout=30)
            except (Exception, SystemExit) as error:
                record["acknowledgement_error"] = str(error)
            write_session_health(session.machine, session.account, record)

    def schedule(self, executor):
        waiting_since = time.monotonic()
        settings = shared_settings()
        while True:
            for session in self.sessions:
                if session.started and session.started.done() and session.started.exception() and not session.closing:
                    error = session.started.exception()
                    self.errors.append(f"{session.id} could not start: {error}")
                    self.fail_session(session, f"could not start: {error}")
                    session.closing = executor.submit(self.close_session, session)
            live = [session for session in self.sessions if not session.closed]
            active = [session for session in live if not session.draining and not session.closing]
            pending = self.queue.pending()
            if not pending and not live:
                return
            in_flight = sum(len(self.queue.assignments[session.id]) > len(session.results) for session in active)
            useful = pending + in_flight
            if self.args.workers:
                useful = min(useful, self.args.workers)
            self.update_demand(useful)
            demands = {run: row["demand"] for run, row in self.pool.runs().items()}
            ready = {machine.name: ready_accounts(machine) for machine in self.machines}
            capacity = sum(map(len, ready.values()))
            target = session_pool.fair_shares(demands, capacity).get(self.id, 0)
            hosted = sum(host_alive(machine) for machine in self.machines) * settings["sessions"]
            if sum(demands.values()) > hosted:
                self.start_next_machine()
            if pending:
                self.acquire(executor, min(target, useful) - len(active), ready)
                for session in reversed(active[target:] if capacity else ()):
                    session.draining = True
                    self.queue.drain(session.id)
            self.peak_sessions = max(self.peak_sessions, sum(not session.closed for session in self.sessions))
            usable = [account for machine in self.machines for account in ready[machine.name]
                      if f"{machine.name}-{account}" not in self.broken]
            if pending and not live and any(ready.values()) and not usable:
                raise RuntimeError(f"Every shared GPU session failed: {'; '.join(self.errors) or 'see the session evidence'}")
            if live or usable:
                waiting_since = time.monotonic()
            elif time.monotonic() - waiting_since > 30 * 60:
                raise RuntimeError(f"No shared GPU session became available in 30 minutes: {'; '.join(self.errors) or 'see `sessions` and `gpus`'}")
            if time.monotonic() - self.polled >= 10:
                self.polled = time.monotonic()
                self.poll(executor)
            time.sleep(2)

    def start_next_machine(self):
        for machine in self.machines:
            error = machine.runtime / "startup-error"
            if error.exists():
                text = error.read_text()
                if text not in self.errors:
                    self.errors.append(text)
                    print(f"{machine.name}: {text}", flush=True)
            if host_alive(machine):
                continue
            if time.monotonic() - self.start_attempts.get(machine.name, -120) >= 120:
                self.start_attempts[machine.name] = time.monotonic()
                start_shared_vm(machine)
            return

    def acquire(self, executor, count, ready):
        machines = sorted(self.machines, key=lambda machine: machine.name not in self.prepared)
        for machine in machines:
            for account in ready[machine.name]:
                if count <= 0:
                    return
                if f"{machine.name}-{account}" in self.broken:
                    continue
                lock = self.pool.claim(machine.name, account, self.id, ROOT)
                if lock is None:
                    continue
                if account not in ready_accounts(machine):
                    lock.close()
                    continue
                session = SharedSession(machine, account, lock, self.id, len(self.sessions) + 1)
                self.queue.add_runner(session.id)
                session.started = executor.submit(self.start_session, session)
                self.sessions.append(session)
                count -= 1

    def abandon(self):
        for session in self.sessions:
            if session.started and not session.started.cancel():
                wait([session.started])
        for session in self.sessions:
            if session.closing is None:
                self.queue.drain(session.id)
                session.failure = session.failure or "The run stopped before this session finished."
                self.close_session(session)

    def release_machines(self):
        for machine in self.machines:
            if machine.name in self.prepared and running(machine):
                guest(machine, f"Remove-Item -LiteralPath '{SHARED_ROOT}\\{self.id}' -Recurse -Force -ErrorAction SilentlyContinue",
                      stdin=subprocess.DEVNULL, timeout=600, check=False)

    def shards(self):
        return [{"vm": session.id, "selectors": self.queue.assignments[session.id], "results": session.results,
                 "complete": session.complete and not session.failure, "failure": session.failure, "evidence": session.evidence}
                for session in self.sessions if self.queue.assignments[session.id]]


def source_archive(hashes, destination):
    listing = "".join(f"{name}\0" for name in hashes).encode()
    run("tar", "--null", "--files-from=-", "--create", "--gzip", f"--file={destination}", cwd=ROOT, input=listing)
    return destination


def shared_fixtures(args):
    fixture_selection(args.names)
    primary = selected(args)
    if primary.runner.exists() or primary.server:
        raise ValueError("Build fixtures from the worktree's primary VM")
    if not (SERVER_BASE / "disk.qcow2").exists():
        raise RuntimeError(f"The shared GPU image does not exist; run `{PROGRAM} setup --server` and `create --server`, or use --warp.")
    if not all(rdp_tools()):
        raise RuntimeError(f"FreeRDP's X11 client or Xvfb is missing; run `{PROGRAM} setup --server`, or use --warp.")
    if not gpu_pool.devices(STATE):
        raise RuntimeError("No GPU pool is configured; see docs/development/windows-vm.md, or use --warp.")
    budget = cpu_pool.configuration(STATE)
    if shared_settings()["cpu_slots"] > min(budget["per_owner_limit"], cpu_pool.capacity(budget)):
        raise RuntimeError(f"A shared GPU VM needs {shared_settings()['cpu_slots']} CPU slots; raise per_owner_limit in "
                           "cpu-pool.json or lower shared.cpu_slots in gpu-pool.json, or use --warp.")
    local = ARTIFACTS / primary.name / ("shared-" + time.strftime("%Y%m%d-%H%M%S"))
    local.mkdir(parents=True, exist_ok=False)
    started = time.monotonic()
    with claim(primary):
        if primary.gpu.exists() and running(primary):
            raise ValueError("Stop the primary GPU VM before preparing a shared fixture run")
        if not running(primary):
            fixture_memory_admission(local, "build-primary", [fixture_memory_gib(MEMORY)])
        boot(primary, gui=False)
        hashes = sync_machine(primary)
        plan = dict(planned_fixtures(primary, args, fixture_directory(True, args.profile)))
        selectors = plan["runs"]
        directory, record = fixture_build(primary, hashes, args.no_build, True, args.profile, controls=plan["controls"])
        provenance = {"revision": run("git", "rev-parse", "HEAD", cwd=ROOT, capture_output=True, text=True).stdout.strip(),
                      "source": hashes, "build": record, "hardware_gpu": True, "shared_sessions": True}
        (local / "provenance.json").write_text(json.dumps(provenance, indent=2))
        (local / "plan.json").write_text(json.dumps(plan, indent=2))
        payload = {"source": source_archive(hashes, local / "source.tar.gz"),
                   "build": export_fixture_build(primary, directory, record, local),
                   "record": record, "hashes": hashes}
        if not args.names:
            stop_machine(primary)
    build_seconds = time.monotonic() - started
    identity = secrets.token_hex(4)
    pool = shared_pool()
    capacity = len(shared_machines()) * len(shared_accounts())
    shards, failure = [], None
    with pool.register(identity, ROOT) as update_demand, \
            fixture_shards.Queue(selectors, [], fixture_durations(), local / "queue.json") as queue:
        shared = SharedRun(args, local, identity, queue, update_demand, payload)
        with ThreadPoolExecutor(max_workers=capacity) as executor:
            try:
                shared.schedule(executor)
            except (Exception, SystemExit) as error:
                failure = str(error)
            finally:
                shared.abandon()
        shared.release_machines()
        shards = shared.shards()
    results, errors = fixture_shards.merge(selectors, shards)
    if failure:
        errors.append(failure)
    plan["shards"] = {row["vm"]: row["selectors"] for row in shards}
    (local / "plan.json").write_text(json.dumps(plan, indent=2))
    (local / "results.jsonl").write_text("".join(json.dumps(row) + "\n" for row in results))
    (local / "shards.json").write_text(json.dumps(shards, indent=2))
    (local / "summary.json").write_text(json.dumps({
        "wall_seconds": time.monotonic() - started, "build_seconds": build_seconds,
        "peak_sessions": shared.peak_sessions, "session_errors": shared.errors, "errors": errors}, indent=2))
    for name in (payload["source"], payload["build"]):
        name.unlink(missing_ok=True)
    print(f"{sum(row['exit'] == 0 for row in results)}/{len(selectors)} passed on up to {shared.peak_sessions} shared GPU sessions. Logs: {local}")
    if errors:
        raise RuntimeError("; ".join(errors))
    (local / "complete").touch()


def sessions(args):
    pool = shared_pool()
    settings = shared_settings()
    accounts = shared_accounts()
    for machine in shared_machines():
        ready = ready_accounts(machine)
        held = pool.holders(machine.name, accounts)
        state = "ready" if ready else "starting" if host_alive(machine) else "stopped"
        card = f", GPU {machine.gpu.read_text().strip()}" if running(machine) and machine.gpu.exists() else ""
        print(f"{machine.name}\t{state}{card}, {len(ready)}/{settings['sessions']} sessions, {len(held)} held")
        for account, row in sorted(held.items()):
            print(f"  {account}\trun {row.get('run')} from {row.get('owner')}")
        error = machine.runtime / "startup-error"
        if error.exists() and state == "stopped":
            print(f"  last error: {error.read_text()}")
    for run_id, row in pool.runs().items():
        print(f"run {run_id}\tdemand {row.get('demand')} from {row.get('owner')} since {row.get('started')}")


def screenshot(args):
    machine = selected(args)
    require_running(machine)
    qmp(machine, "screendump", filename=str(args.file.resolve()), format="png")


def reset(args):
    machine = selected(args)
    if machine.runner.exists() and json.loads(machine.runner.read_text()).get("worktree") != str(ROOT.resolve()):
        sys.exit(f"The {machine.name} runner belongs to another worktree.")
    if running(machine) or machine.runner.exists() and runner_reserved(machine):
        sys.exit(f"Stop the VM first with `{PROGRAM} --vm {machine.name} stop`.")
    remove(machine.directory)


def list_machines(args):
    for machine in machines():
        state = f"running, ssh port {machine.ssh_port.read_text().strip()}" if running(machine) else "stopped"
        if state != "stopped" and machine.gpu.exists():
            state += f", GPU {machine.gpu.read_text().strip()}"
        print(f"{machine.name}\t{state}")
    if process(LEGACY_PID, "qemu-system-x86"):
        print("(unnamed VM from an older windows-vm.py)\trunning")


def destroy(args):
    server = getattr(args, "server", False)
    scope = [machine for machine in machines() if machine.server] if server else machines()
    active = [machine.name for machine in scope + [server_installer(), *([] if server else [installer()])] if running(machine)]
    if not server and process(LEGACY_PID, "qemu-system-x86"):
        active.append("(unnamed VM from an older windows-vm.py)")
    if active:
        sys.exit(f"Stop these VMs first: {', '.join(active)}")
    if server:
        run("ssh-keygen", "-R", "capycanvas-windows-server", "-f", KNOWN_HOSTS, check=False,
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        for path in [machine.directory for machine in scope] + [SERVER_INSTALL, SERVER_BASE, SERVER_PASSWORD]:
            remove(path)
        return
    for path in (INSTALL, BASE, SERVER_INSTALL, SERVER_BASE, VMS, LEGACY_RUN, STATE / "run.partial", FIRMWARE, PASSWORD, SERVER_PASSWORD, KNOWN_HOSTS):
        remove(path)


def positive_int(value):
    number = int(value)
    if number < 1:
        raise argparse.ArgumentTypeError("Expected a positive integer")
    return number


def fixture_timeout(value):
    minutes = int(value)
    if not 1 <= minutes <= 1440:
        raise argparse.ArgumentTypeError("Fixture timeout must be 1–1440 minutes.")
    return minutes


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--vm", help="VM name (default: $CAPYCANVAS_VM or the worktree directory name)")
    commands = parser.add_subparsers(required=True, metavar="command")

    def command(name, handler, summary):
        subcommand = commands.add_parser(name, help=summary, description=summary)
        subcommand.set_defaults(handler=handler)
        return subcommand

    setup_command = command("setup", setup, "install host packages, grant KVM access and download the ISO")
    setup_command.add_argument("--iso", type=Path, help="use this ISO instead of downloading one")
    setup_command.add_argument("--server", action="store_true", help="prepare the Windows Server image of the shared GPU VMs")
    create_command = command("create", create, "install Windows and the build tools into the base image")
    create_command.add_argument("--gui", action="store_true", help="show the installer display in a window")
    create_command.add_argument("--server", action="store_true", help="install the Windows Server image of the shared GPU VMs")
    start_command = command("start", start, "boot the VM, creating it from the base image if needed")
    start_command.add_argument("--gui", action="store_true", help="show the VM display in a window")
    start_command.add_argument("--gpu", nargs="?", const="auto", help="claim one free GPU, or the specified pool PCI address")
    command("gpus", gpus, "show the hardware GPU pool and its reservations")
    command("sessions", sessions, "show the shared GPU VMs, their sessions and the runs sharing them")
    command("shared-host", shared_host, argparse.SUPPRESS)
    command("runners", runners, "show CPU runner reservations").add_argument(
        "--reset", action="store_true", help="delete this owner's stopped runner disks")
    command("gpu-check", gpu_check, "run the hardware renderer and Windows D3D12 tests").add_argument(
        "--no-build", action="store_true", help="reuse a validated hardware app build")
    command("stop", stop, "shut the VM down")
    command("list", list_machines, "list the VMs made from the base image")
    command("ssh", ssh, "open a PowerShell session or run a command in the VM").add_argument(
        "command", nargs=argparse.REMAINDER)
    command("sync", sync, f"copy the working tree to {GUEST_REPO}, keeping build outputs")
    command("check", check, "sync, build and run the tests that need no GPU").add_argument(
        "--debug", action="store_true", help="use unoptimized Debug builds and tests (default: Release)")
    fixture = command("fixtures", fixtures, "sync, build and run UI fixtures, by default in shared GPU VM sessions")
    fixture.add_argument("names", nargs="*", help="fixtures to run, e.g. layers or header:pen (default: all)")
    fixture.add_argument("--no-build", action="store_true", help="reuse a validated build for the selected adapter mode")
    fixture.add_argument("--profile", choices=("release", "dev-perf"), default="release",
                         help="Rust build profile (default: release; dev-perf enables incremental development builds)")
    fixture.add_argument("--workers", type=positive_int, help="use at most N shared GPU sessions (default: a fair share of those free)")
    fixture.add_argument("--hardware-gpu", action="store_true", help="claim a whole GPU and console desktop for one VM")
    fixture.add_argument("--warp", action="store_true", help="run on the WARP CPU pool, sending marked rows to one GPU VM")
    fixture.add_argument("--runners", type=positive_int, help="run WARP fixtures on up to N pooled CPU VMs (implies --warp)")
    fixture.add_argument("--keep-runners", action="store_true", help="retain CPU runners and their reservations after a sharded run")
    fixture.add_argument("--timeout-minutes", type=fixture_timeout, default=20,
                         help="deadline for each fixture, 1–1440 minutes (default: 20)")
    command("screenshot", screenshot, "save the VM display as a PNG").add_argument("file", type=Path)
    command("reset", reset, "delete the VM; the next start makes a fresh one from the base image")
    command("destroy", destroy, "delete the base image and every VM, keeping the downloaded ISO").add_argument(
        "--server", action="store_true", help="delete only the Windows Server image and the shared GPU VMs")
    args = parser.parse_args()
    try:
        args.handler(args)
    except (OSError, ValueError, RuntimeError) as error:
        sys.exit(str(error))
    except subprocess.CalledProcessError as error:
        sys.exit(f"Exit status {error.returncode}: {shlex.join(map(str, error.cmd))}")


if __name__ == "__main__":
    main()
