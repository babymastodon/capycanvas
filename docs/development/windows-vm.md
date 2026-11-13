# Windows VMs on Linux

[Windows development](windows.md) · [Developer guide](README.md)

`tools/windows-vm/windows-vm.py` runs Windows evaluation VMs under QEMU/KVM. A
worktree's own VM runs Windows 11 Enterprise on Windows' software D3D12 adapter
(WARP) and builds the app. UI fixtures run by default in
[shared GPU sessions](#shared-gpu-sessions), where agents share Windows Server VMs
that each own one pool card. Choose a fixture mode by what you need to test:

| Mode | Command | Use it for |
| --- | --- | --- |
| Shared GPU sessions (default) | `fixtures [names]` | UI structure, shared state, injected input and rendered journeys, from one row to the full matrix |
| WARP CPU runners | `fixtures --warp [names]`, `fixtures --runners N [names]` | Shader and frame-path changes that must complete on WARP, the 8 GiB memory limit of a runner, or selections without marked rows when no card is free |
| Exclusive GPU VM | `fixtures --hardware-gpu [names]`, `gpu-check` | The renderer suite, Windows 11, a real console desktop or a whole card |

The table below describes what the worktree's own VM can run without a GPU:

| Runs in a VM | Needs a hardware GPU |
| --- | --- |
| `check`: the build, native input tests and Rust unit tests, including the GPU-backed `layer-host` tests and the D3D12 document, tab and filter-package tests | `layer-render-wgpu` unit tests, which time out and exceed pixel tolerances on WARP |
| `fixtures --warp`: native UI fixtures except marked registry rows | ignored `layer-windows` tests that remove devices, the HDR document test and the color import/export round trip |
| | `tools/performance` scripts, PresentMon, physical pens and touch screens, HDR output, multiple displays and visual parity captures |

Requirements: x86_64 Linux with KVM, Python 3, about 150 GB of free disk space for
the base image plus 10–30 GB for each VM, and network access. `setup` installs
packages automatically on Fedora, Ubuntu/Debian and Arch.

```sh
tools/windows-vm/windows-vm.py setup     # packages, KVM access, ISO download
tools/windows-vm/windows-vm.py create    # unattended Windows and toolchain install
tools/windows-vm/windows-vm.py check     # sync, build and run GPU-free tests
tools/windows-vm/windows-vm.py fixtures --warp  # sync, build and run the UI fixtures on WARP
```

`setup` downloads the evaluation ISO into your downloads folder unless it is
already there; `--iso` uses an existing file. `create` installs Windows, OpenSSH,
Visual Studio Build Tools, Rust, NuGet and PowerShell 7 into a read-only base
image. It needs no input and usually takes 20–40 minutes; pass `--gui` to watch it.

## VMs

Every other command acts on one VM made from the base image. `--vm NAME` or
`CAPYCANVAS_VM` selects it; the default is the worktree's directory name, so each
worktree has its own VM and its own `C:\capycanvas`. The first `start` creates the
VM as a copy-on-write disk over the base image and prepares it with
[`prepare.ps1`](../../tools/windows-vm/prepare.ps1): PowerShell 7 if the base lacks
it, Japanese Basic input (the `ime` fixture adds the Microsoft Japanese IME only
while it runs), a 2560 × 1600 display at 150 % scale, the dark theme the
fixtures were written against, and one restart. This takes a few
minutes; later starts take under a minute. Several VMs can run at once, each with
its own disk, TPM, SSH port and desktop session.
Startup keeps local test passwords from expiring and repairs an expired autologon
before using the desktop.

```sh
tools/windows-vm/windows-vm.py --vm review start
tools/windows-vm/windows-vm.py list
```

`check` boots the VM when needed, copies the working tree, including uncommitted
and untracked files, to `C:\capycanvas`, and runs
[`test-without-gpu.ps1`](../../apps/layer-windows/scripts/test-without-gpu.ps1):
the optimized Release build (`--debug` for Debug), the native input tests, and the
shared and Windows Rust unit tests, with the `layer-host` GPU tests and the
ignored `d3d12_` document tests on WARP. Builds and tests use the selected
configuration and the [pinned test-only WARP runtime](windows.md#test).
Each check uses a fresh
`CAPY_STORAGE_DIR` under `artifacts/windows/test-storage`; keep it isolated across runs.
Build outputs persist between syncs;
each sync sends only files that changed since that VM's last sync, stamped with
the current time, so incremental builds stay correct when a VM switches trees.
Build commands update stable Rust only when the active compiler is older than the
workspace's `rust-version`, then verify it before running Cargo.
`sync`, `check` and `fixtures` lock their VM, so run parallel work on separate
VMs. Run other commands from the synced tree, for example strict Clippy:

```sh
tools/windows-vm/windows-vm.py ssh 'cd C:\capycanvas; cargo clippy --locked -p layer-windows --all-targets -- -D warnings'
```

| Command | Effect |
| --- | --- |
| `start [--gui]`, `stop` | Boot or shut down the VM, creating it first if needed. |
| `list` | Show the VMs and which are running. |
| `ssh [command]` | Open PowerShell, or run a command, as the `capy` administrator. |
| `sync` | Copy the working tree without building. |
| `fixtures [name ...]`, `sessions` | Run UI fixtures in [shared GPU sessions](#shared-gpu-sessions); show those VMs, their sessions and the runs sharing them. |
| `fixtures --warp [name ...]`, `fixtures --runners N [name ...]`, `runners [--reset]` | Shard CPU fixtures over reserved runners, with marked rows on one owned GPU VM; list reservations or delete stopped runners owned by this worktree. |
| `screenshot <file.png>` | Save the VM display. |
| `reset` | Delete the VM; the next `start` makes a fresh one. |
| `destroy` | Delete the base image and every VM; the ISO stays. Stop the VMs first. |

Guest PowerShell scripts stream over SSH stdin through a fixed-size loader, so
multiline Unicode scripts do not depend on the Windows command-line length limit.

State lives in `~/.local/share/capycanvas/windows-vm` (`CAPYCANVAS_VM_DIR`): the
base image in `base` and the VMs in `vms`. `CAPYCANVAS_VM_CPUS` (default up to 8),
`CAPYCANVAS_VM_MEMORY` (default `16G`) and `CAPYCANVAS_VM_DISPLAY` (default
`2560x1600`) apply when a VM starts. The generated account password is in
`password` there. Setup and creation restrict the state directory to its owner;
the password and unattended-install answer file are readable and writable only
by that owner. These local test credentials remain in plaintext for unattended
Windows setup and console login. A successful installation removes the seed
directory and ISO; failed installation files remain inside the private state
directory for diagnosis.

Windows Update is disabled so the VMs keep the ISO's build. The evaluation
licence expires 90 days after `create`; stop every VM, then run `destroy` and
`create` to renew it.

## Shared GPU sessions

Set up the shared image once per host. It needs FreeRDP's X11 client and Xvfb;
without root, unpack those packages under `<state>/rdp-client/usr`.

```sh
tools/windows-vm/windows-vm.py setup --server   # packages and the Windows Server ISO
tools/windows-vm/windows-vm.py create --server  # about 20 minutes, unattended
```

Then `fixtures` needs no reservation, from any worktree and any number of agents
at once:

```sh
tools/windows-vm/windows-vm.py fixtures smoke navigator
tools/windows-vm/windows-vm.py fixtures --workers 4 layers
tools/windows-vm/windows-vm.py sessions
```

The worktree's own VM builds the hardware app and any control fixtures. The VMs
`shared-gpu-1` and `shared-gpu-2` then run the fixtures, one Remote Desktop session
per fixture at a time. Each VM runs Windows Server 2025 and holds one whole pool
card and eight CPU slots (32 vCPU / 64 GiB), so both leave eight WARP runners in a
96-vCPU pool. Fifteen sessions fit on a VM; a sixteenth exhausts desktop resources.
`gpu-pool.json` can set `{"shared": {"vms", "sessions", "cpu_slots", "idle_minutes"}}`;
each VM is its own owner in `cpu-pool.json`, so `per_owner_limit` must allow its `cpu_slots`.

A run starts the first VM when it needs one and the second only when total demand
exceeds the first. A VM stops after 10 minutes with no leased session, releasing its
card and slots; `--vm shared-gpu-N stop` stops an idle one sooner. `start` warms one.
If no card or slot is free, the run keeps waiting for 30 minutes and then fails
with the reason; it never falls back to WARP.

Running fixture lists share sessions fairly: a run that needs little gets all it
needs and larger runs split the rest. A run grows when sessions free up, and it
gives one back after that session's current fixture when another run needs it, so
a short list started during a full round begins within minutes. `--workers N` caps
a run. Each session is its own Windows account, desktop, clipboard and user
settings, with its own source tree and writable directories. Immutable build
files are read-only hardlinks to one shared copy per run. The normal `check`
suite runs `test-session-view.ps1` to verify sharing and write isolation.

Each fixture must leave hardware-adapter evidence and report 150% scale. Sessions
run at 2560 × 1600 with the same three-times `CAPY_WAIT_SCALE` as WARP, because 15
apps starting together need about 14 s for their first shaders. Sessions are virtual
displays with no vsync target, so they do not measure frame rate or presentation.

The merged output under `<artifacts>/<primary>/shared-<timestamp>` holds `plan.json`,
`results.jsonl`, `shards.json`, `summary.json` and `provenance.json`, with each
lease's evidence and crash dumps under `sessions/`. A lost session reports its
claimed selector as missing, and its account takes no more work in that run.

The evaluation licence lasts 180 days and Remote Desktop's unlicensed grace period
120 days from `create --server`. To renew, wait until `sessions` shows no runs, stop
both VMs, then run `destroy --server` and `create --server`.

## CPU runner pool

Use pooled CPU runners for WARP fixture lists with more than a few selectors:

```sh
tools/windows-vm/windows-vm.py runners
tools/windows-vm/windows-vm.py fixtures --runners 3 button-feedback:default navigator:default popup:default
tools/windows-vm/windows-vm.py fixtures --runners 6 --no-build layers palettes proof header selection editor
```

The primary VM builds each required software or hardware app once, with only the
Feedback, Popup or LayerDrop controls needed by that mode's planned selectors. The helper ships their runtime files, assets and build record to owned
`<primary>-r1` … runner VMs, validates every recorded hash, and runs fixtures in parallel.
Idle VMs claim remaining selectors once, longest recorded duration first. Bare
names and `:default` keep the same selection rules as ordinary `fixtures`.

`cpu-pool.json` in the VM state directory sets `vcpu_budget`,
`memory_budget_gib`, `runner_cpus`, `runner_memory_gib` and `per_owner_limit`.
Defaults reserve half the host's logical CPUs and physical memory for runners,
with 4 vCPU / 8 GiB per runner and at most six per worktree, limited by capacity.
Pool size settings apply to CPU runners independently of ordinary VM environment
overrides. Change the size or budget settings only after all runner reservations
are free; `per_owner_limit` can change while runners are reserved.

Slot locks are host-wide, including across VM state directories. QEMU inherits
each reservation descriptor and holds it until shutdown or a crash, even if the
launcher disconnects. A full pool supplies fewer runners when possible; zero
available runners fails clearly. Existing ordinary or foreign VMs are never
adopted as runners. Concurrent commands cannot use the same runner. CPU runners
belong to their creating worktree; run each concurrent round from its own worktree.

The merged output under `<artifacts>/<primary>/shards-<timestamp>` records the
full plan, each shard's evidence path and provenance, every result and total wall
time. A missing selector or shard fails the run. Runners stop on success or
failure; `--keep-runners` retains their reservations for another round. Their
prepared disks persist. Stop retained runners with `--vm <runner> stop`, and use
`runners --reset` from the owning worktree to delete this primary VM's stopped
runner disks once that worktree no longer runs rounds.

Crash dumps land in each fixture run's `wer/dumps` evidence directory; the runner restores the per-app LocalDumps key's original values and registry types before retrieval.

On `--warp` or `--runners`, marked registry rows run on one pool GPU VM at 150% scale.
It holds two CPU slots (8 vCPU / 16 GiB), leaving at most 22 CPU runners
in a 96-vCPU pool. Admission requires `MemAvailable` of at least the total
memory of VMs being started plus 40 GiB. The card and both slots are always
released, even with `--keep-runners`.
If no card is free, the run stops before any fixture starts; check `gpus`.

CPU runners use WARP and cannot claim GPUs. They check UI structure, shared state
and injected input; hardware rendering and performance still need the checks
below.

## Hardware GPU pool

An agent claims one free GPU when its VM starts and releases it when the VM shuts
down. Cards stay bound to VFIO between guests. Assignment happens at boot;
hot-attach is not implemented. Ordinary `start` and `check` keep their
software-adapter behaviour unless hardware mode is requested.
Keep a native GPU outside the pool for the desktop and Linux applications to
share. A passed-through card belongs exclusively to its guest until shutdown.

```sh
tools/windows-vm/windows-vm.py gpus
tools/windows-vm/windows-vm.py --vm renderer-review start --gpu
tools/windows-vm/windows-vm.py --vm renderer-review sync
tools/windows-vm/windows-vm.py --vm renderer-review ssh 'Get-PnpDevice -Class Display'
tools/windows-vm/windows-vm.py --vm renderer-review gpu-check
tools/windows-vm/windows-vm.py --vm renderer-review fixtures --hardware-gpu canvas-editing documents:default layers:default
tools/windows-vm/windows-vm.py --vm renderer-review stop
```

Use your worktree's VM or an explicitly owned name. `start --gpu PCI_ADDRESS`
requests a particular pool card; without an address it chooses any free card.
`gpus` shows free, reserved, externally used and unavailable cards. QEMU holds the
reservation lock itself, so closing a terminal does not release a running VM's
card. The pool also respects VMs launched outside this helper. A full pool fails
with a diagnostic instead of switching to WARP.

GPU VMs run as user systemd services with unlimited locked memory. The VM remains
available for SSH and screenshots if a startup check fails. `stop` requests a
clean guest shutdown; a GPU shutdown timeout leaves the VM and reservation
running for diagnosis. Do not kill QEMU or reset PCI devices to free a card.
Some GPUs have reset faults that require host recovery after an unclean teardown.

`gpu-check` builds the normal Release app and runs the shared renderer suite and
the Windows D3D12 document suite against an actual hardware D3D12 adapter. It
rejects empty test selections. The renderer suite runs eight test threads (about
20 minutes instead of 80); tests that fail under that load run again alone, the
suite passes only if each passes alone, and `results.json` lists them as
`parallel_failures`. `fixtures --hardware-gpu` runs the normal app with
software adapters disabled and requires actual hardware-adapter evidence in each
selected rendered journey. Use explicit render fixtures; intentional GPU-startup
failure journeys belong in the ordinary fixture matrix. Hardware startup
configures the assigned NVIDIA card as the primary desktop
at the configured display size and 120 Hz. The emulated adapter remains a
secondary management display. The helper verifies the actual adapter, primary
flag, size, refresh and monitor scale; a renderer adapter alone is insufficient.
Both commands accept
`--no-build` only when synced build inputs and executable/DLL hashes match. These
checks do not qualify physical pens, HDR output, display combinations or frame
rate targets.

### Hardware desktop

Headless NVIDIA workstation cards use the installed signed driver's NVAPI EDID
[emulation](https://www.nvidia.com/content/Control-Panel-Help/vLatest/en-us/mergedProjects/nvwks/To_force_an_EDID_at_a_display_connection.htm).
Hardware startup selects one connector on the single assigned GPU,
sets the configured `CAPYCANVAS_VM_DISPLAY` size at 120 Hz and makes it primary.
It reuses an exact helper EDID and refuses unrelated displays or EDIDs rather
than replacing them. Only helper-owned EDIDs can be updated when the configured
size changes. Unsupported timings fail before changing the display. Other GPU
vendors need a supported desktop setup before using this hardware workflow.

The EDID override lasts for one boot session. The helper installs
`capycanvas-hardware-display`, an elevated interactive logon task, under the
private `capy` account. It stores the script and connector ownership under
`C:\capycanvas-vm-display`, outside the synced source tree. Every hardware start,
including an already-running VM, restores and verifies the desktop under the
VM lock; the logon task restores it after a guest restart. No GPU reset, driver
replacement or guest reboot is required for desktop configuration.

`receipt.json` in that guest directory and `display.json` in the VM's host state
record the adapter, Windows display device, primary flag, origin, dimensions,
refresh, effective scale/DPI, driver and boot time. Configuration preserves the
monitor's Windows scale and reports its actual value; a newly attached monitor
can choose a different scale from the emulated display. QEMU screenshots still
show the secondary emulated output. Use composed app captures on the hardware
monitor when validating visible rendering.

A 120 Hz desktop is a display configuration, not evidence that the app produces
120 changing frames per second. Performance and physical input still require
their normal measurements. Software VMs keep their existing display path.

### Desktop presentation for performance

A hardware rendering adapter alone does not establish a hardware desktop.
Cross-adapter presentation through the emulated output can dominate buffer
acquisition. Verify the monitor containing the app before interpreting window
motion rates.

Run `apps/layer-windows/scripts/probe-displays.ps1` in the guest's interactive test
desktop to verify the adapter, primary flag, coordinates, resolution and refresh.
Check the app is wholly on that GPU's monitor. QEMU screenshots still show the
emulated output; use the owned app's composed capture on the hardware monitor.
Do not enable `-ConfigureVmDisplay` when it would replace the selected GPU mode
with the ordinary fixture mode.

Measure buffer-acquisition waits separately from render calls and GPU execution.
Keep displayed frames, renderer submissions and changing native bounds separate.
The [column-resize diagnostic](../performance/top-tier.md#windows-gpu-vm-column-resize)
records both desktop configurations. These remain workstation diagnostics, not
reference-device or physical-input qualification.

Offscreen `layer-bench` and `raster_workloads` bypass desktop presentation. Use the
[GPU benchmark guide](gpu-raster-benchmarks.md), select the actual hardware D3D12
adapter through `LAYER_GPU_INDEX`, and retain adapter/backend and binary hashes.
Serialize benchmarks, builds and correctness suites. An idle low-power GPU state
is normal; record clocks, power and PCIe link under load. Use vendor GPU memory
reporting rather than the truncated `Win32_VideoController.AdapterRAM` field.

### Prepare a host

The host needs an IOMMU, isolated PCI groups, QEMU VFIO support, `fuser` (the
`psmisc` package), `acl` (`setfacl`), `driverctl` and a systemd user manager. Install Windows and its
toolchain with the normal setup first. Keep the base image and VM overlays on a
local SSD. Set `CAPYCANVAS_VM_DIR` before `setup` when choosing a different state
directory; `/mnt/localssd` is suitable scratch storage when available. Never move
or copy a running VM's disk.

Choose spare GPU addresses from `lspci -nnk`. Preserve the host's boot/display GPU.
The setup refuses boot GPUs, connected displays and IOMMU groups containing
other devices. Close graphics clients and release the selected GPU's audio
device first; this can require ending the desktop session and temporarily
stopping the host sound-state service. Run from an independent SSH or console
session when the desktop must stop. Detaching a terminal is not sufficient when
its server still belongs to a graphical-session systemd scope.

The host script prints a read-only plan by default. Supply your own user, state
path and PCI addresses:

```sh
python3 tools/windows-vm/gpu-host.py PCI_ADDRESS ... \
  --user "$USER" --state-dir "$HOME/.local/share/capycanvas/windows-vm" \
  --artifacts /path/on/local-ssd/windows-vm-results \
  --guest-driver /path/on/local-ssd/windows-driver-bundle
```

After reviewing the plan and making the devices idle, run the same command with
`sudo` and `--apply`. The script makes per-device driver overrides persistent,
sets permissions for the selected user's VFIO groups, and raises the user
manager's locked-memory limit without restarting it. Already bound VFIO cards
are left running; persistence is written without reprobe or unbind. It never
stops a desktop, kills a graphics client or changes boot-loader settings.

Provide a vendor-signed Windows INF bundle for the pool's GPUs. A Windows
administrator can export an installed package with
`pnputil /export-driver oemNUMBER.inf OUTPUT_DIRECTORY`. The helper imports that
bundle on each VM's first hardware start and reuses its driver store thereafter.
An already installed or newer driver is accepted only when a present display
device has a healthy, signed vendor driver.
Without a bundle, install the guest's vendor driver manually before GPU tests.
Driver binaries and PCI addresses stay in local state, not the repository.

The generated `gpu-pool.json` lives in the VM state directory. It records the
cards, driver bundle, artifact path and host-sized resource defaults. Defaults
reserve a host share and divide CPU/RAM among the pool's possible concurrent
guests, capped at 8 vCPUs and 16 GiB per VM because every worktree primary, GPU
VM and CPU runner shares the host. Each guest uses one CPU socket.
`--cpus` and `--memory` on host setup, or the existing `CAPYCANVAS_VM_CPUS` and
`CAPYCANVAS_VM_MEMORY` environment variables, override them. Keep successful VMs
warm during iteration to reuse Cargo, NuGet and native build caches; stop them
when the task releases its GPU.

Large evidence is copied to the configured artifact directory;
`CAPYCANVAS_VM_ARTIFACTS` overrides it. `CAPYCANVAS_VM_RUNTIME` optionally selects
another directory for small PID/socket files. Keep runtime paths short enough
for Unix sockets. Images, build caches, captures and test logs belong on disk,
not a RAM-backed `/tmp`.

## UI fixtures on the software adapter

For CPU rows, `fixtures --warp` syncs and builds Release with `build.ps1 -SoftwareAdapterTests` into
`artifacts\windows\SoftwareAdapter`, and runs
[`run-fixtures.ps1`](../../tools/windows-vm/run-fixtures.ps1) in the VM's desktop
session through a scheduled task, because SSH sessions cannot use UI Automation
or inject input. The runner works like a contributor at an unlocked desktop: one
fixture at a time, from a visible console that gives each fixture foreground
rights. It stops only CapyCanvas processes launched during that run from the
selected executable and records each exit code, duration and final error. It
sets `CAPY_WAIT_SCALE=3`, which lengthens every `CapyUia.ps1` wait for the slower
adapter. The document fixture uses that bound for process cleanup and checks that
the window continues answering native messages while the GPU queue drains.
Software-adapter shutdown timing does not establish hardware performance.

For repeated code edits, `fixtures --profile dev-perf` selects optimized
incremental Rust compilation. Its first build prepares a separate Cargo cache.
App outputs and build records are separate for each profile and adapter mode;
`--no-build` validates the selected profile as well as source and binary hashes.
Control fixtures reuse the same build record and validate runtime resources as
well as their executables; shipped builds never invoke a control rebuild.
The per-VM lock serializes profile switches, which reuse the same adapter mode's
native Release intermediates. Ordinary `fixtures`, `check` and `gpu-check` retain
their Release defaults.

```sh
tools/windows-vm/windows-vm.py fixtures --profile dev-perf --hardware-gpu canvas-editing:smoke canvas-editing:smoke-light
```

Focused canvas selections are `smoke`, `preview` and `controls`, each with a
`-light` variant. Use `canvas-editing:default canvas-editing:light` for just the
full dark/light pair. A bare `canvas-editing` includes the focused variants too.

Name fixtures (`layers`) or single variants (`header:pen`). A bare name includes all its variants;
`localization:default` selects only the default dark, normal-text run. Split long
WARP matrices with `fixtures --runners N` within the [CPU pool](#cpu-runner-pool) budget.
On `--warp`, the registry marks hardware rows under the [validation rule](testing.md#windows-validation-scope); new rows default to CPU there. With no names it runs `shortcuts`
and every fixture that launches its own app, once with its defaults and once for
each switch and each other `ValidateSet` choice, including document GPU recovery
and exhausted recovery. The isolated removal hook drains rendering and
presentation before removing the real device; raw in-flight GPU loss remains
a hardware check. `--no-build` verifies the synced build inputs, including embedded Fluent catalogs,
and executable/DLL hashes before reusing a build. Fixture-only changes can reuse it; changes to Rust, native code
or build scripts require rebuilding.

Each run writes its expected variants to `plan.json`. Empty selections, missing
results, a stopped desktop task and startup or fixture deadlines fail the run.
The shared fixture reader uses bounded retries for brief trace-file I/O locks;
malformed snapshots still fail immediately.
Restore changed Windows user settings from the runner, with forced-termination
coverage in `test-fixture-routing.ps1`.

Follow the [Windows validation scope](testing.md#windows-validation-scope).
The `smoke` alias selects 40 fixed variants; add affected families in the same
command. With `--warp` or `--runners`, its `drawers` and `drawers:light` rows
require one free exclusive pool card and two CPU slots. If no card is free, the
run stops before any fixture input. Plain `fixtures` uses shared GPU sessions.
The runner removes duplicate selections. For the full matrix, omit all fixture
names:

```sh
tools/windows-vm/windows-vm.py fixtures smoke navigator
tools/windows-vm/windows-vm.py fixtures
```

Check that the merged results cover every
selector in `plan.json`, and keep first-attempt failures even when a diagnostic
rerun passes.

Results, logs, desktop failure screenshots and per-fixture evidence are copied to
`artifacts/windows-vm/<vm>/<run>/`. Evidence includes the disposable profile,
fixture captures and owned process traces. `provenance.json` records the source
revision, synced file hashes and binary hashes. Preserve both desktop and
fixture captures when diagnosing a rendering failure; they may use different
capture APIs.

Each fixture has a 20-minute deadline. For longer language matrices, pass
`fixtures localization --timeout-minutes 90`; the accepted range is 1–1440
minutes. `layers:Paper` walks both themes and needs `--timeout-minutes 60` on
the software renderer. `plan.json` records that deadline and the monitor uses it for each
expected result. Input waits and assertions keep their existing limits.

Only builds with the `software-adapter-tests` feature of `layer-windows` accept a
software adapter, and only while `LAYER_TEST_SOFTWARE_GPU` is set. Shipped builds
never enable the feature and still refuse CPU adapters with the shared
unsupported-GPU message, which names the software renderer. UI traces name the adapter in `windows_adapter`, for example
`Microsoft Basic Render Driver (Cpu)`.

A WARP pass shows that the UI structure, shared state, injected mouse, pen and
touch routing, persistence, windows and device-loss handling behave as the
fixture expects. It does not validate hardware drivers, frame pacing or latency,
physical pens and touch screens, HDR output, or mixed-DPI and multiple displays.
WARP timings mean nothing; run `tools/performance` scripts and PresentMon captures
on hardware. `exercise-package.ps1` is not run because the VM copy has no Git
metadata for `package.ps1`.
