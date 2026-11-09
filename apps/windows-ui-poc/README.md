# Windows UI POC

VM A/B diagnostics compare a retained Rust UI and the current client's WinUI architecture over the same D3D12 canvas. VM results do not qualify hardware tier targets.

Build on the guest from the synced `C:\capycanvas` checkout:

```powershell
.\apps\windows-ui-poc\scripts\build.ps1
```

Run benchmarks from the assigned Linux checkout:

```sh
apps/windows-ui-poc/scripts/vm-bench.sh --vm capycanvas6-poc-warp --profile normal
```

The host wrapper checks memory admission, syncs, builds Release, verifies pinned PresentMon 2.3.0, starts an interactive task in a private test desktop, retrieves the run and stops its VM. Only `capycanvas6-poc-gpu` and `capycanvas6-poc-warp` are accepted. `--already-started` uses an owned VM already started by the caller; the wrapper still stops it. No VM configuration or pool changes are made.

Select a checkpoint smoke with `--arm winui --workload startup --workload paint-still --workload divider`; omit those flags for the full ABBA matrix.

WARP uses PresentMon runtime events with display/GPU/input tracking disabled. Its PresentMode and display timings remain unverified. GPU runs require full PresentMon display metrics. Desktop Duplication and pen-delivery checks apply to both VMs.

Run the completed arms with `--vm capycanvas6-poc-gpu --profile normal`, `--vm capycanvas6-poc-gpu --profile lowcpu`, or `--vm capycanvas6-poc-warp --profile normal`. Hardware startup requires a free pool GPU. Verify the NVIDIA primary at 120 Hz and effective DPI before interpreting hardware diagnostics.

Observer invocation on the guest:

```powershell
.\apps\windows-ui-poc\target\release\poc-observer.exe run --arm retained --exe .\apps\windows-ui-poc\target\release\poc-retained.exe --workload divider --contacts 3 --seconds 6 --warmup 1 --profile normal --out C:\capycanvas\artifacts\windows-ui-poc\manual -- --canvas-load light --thumbnails off
```

Run observer commands only on the private desktop created by `apps/layer-windows/scripts/test-desktop.ps1`. `--no-capture` collects injections and process data without Desktop Duplication. Thumbnails default off; the benchmark enables them only for wheel/touch/stall-scroll. The sole load is `light`, with `slow` reserved for the slowcanvas workloads. The matrix uses dark theme only.

The observer supplies `--ready-event` and starts input after the arm signals that its input routing is ready; standalone app launches omit that option.

Panel hosts attach the configured handle, register independent input handlers, then call `poc_canvas_start_panel`. The first completed frame supplies presentation readiness.

Run guest functional checks with `scripts/check-functional.ps1 -Vm capycanvas6-poc-warp -Arm winui -Out C:\capycanvas\artifacts\windows-ui-poc\functional`; it creates the private desktop when needed. Use `-BenchmarkResults` to reuse matching completed contacts. Run `scripts/test-functional-pixels.ps1 -Out C:\capycanvas\artifacts\windows-ui-poc\pixel-tests` for the PNG algorithm checks. Review the saved text, DPI, popup and selected IME crops before interpreting their results.

`poc-observer pen-check --arm winui --exe <PocWinUI.exe> --samples 2400 --out <directory>` checks exactly 2,400 accepted samples. `poc-observer snapshot --hwnd <decimal HWND> --pid <PID> --out <PNG> [--rect x,y,width,height | --screen]` captures the owned private client or primary display through Desktop Duplication.

Host builds use `CARGO_TARGET_DIR=/mnt/localssd/capycanvas6-poc/target`. Raw runs belong under `/mnt/localssd/capycanvas6-poc/results/<vm>/<run>/`, outside Git. Rebuild reports without a VM:

```sh
python3 apps/windows-ui-poc/scripts/report.py /mnt/localssd/capycanvas6-poc/results --out artifacts/windows-ui-poc
python3 -m unittest discover -s apps/windows-ui-poc/scripts -p 'test_*.py'
```

Reports use displayed geometry for motion, UI markers for input echoes and all-content changes, and canvas markers for consumed samples. Marker payloads keep their specified positions and use a one-pixel framing guard to reject blank window contents. Missing measurements remain null. JSON preserves individual contacts, pooled distributions, order slots, censored/unmatched counts, capture failures and marker samples. An observer or capture failure makes the benchmark fail.

Contacts keep six measured seconds. The injector samples the trajectory at actual injection time and skips overdue slots. Captured pen contacts wait for the final accepted sample's displayed marker after settling, failing if it is not observed within ten seconds. Reports retain achieved input rates, skipped slots, input-limited contacts below 90% of nominal, capture-limited contacts, and observer thread CPU. Receipt timing uses source order only after sample tuples and delivery counts pass validation.

The locked dependency graph declares these licences, including platform-specific dependencies:

| Crate | Version | Licence declaration |
| --- | --- | --- |
| allocator-api2 | 0.2.21 | MIT OR Apache-2.0 |
| arrayvec | 0.7.8 | MIT OR Apache-2.0 |
| autocfg | 1.5.1 | Apache-2.0 OR MIT |
| bit-set | 0.10.0 | Apache-2.0 OR MIT |
| bit-vec | 0.9.1 | Apache-2.0 OR MIT |
| bitflags | 2.13.2 | MIT OR Apache-2.0 |
| bumpalo | 3.20.3 | MIT OR Apache-2.0 |
| bytemuck | 1.25.2 | Zlib OR Apache-2.0 OR MIT |
| bytemuck_derive | 1.12.1 | Zlib OR Apache-2.0 OR MIT |
| cfg-if | 1.0.5 | MIT OR Apache-2.0 |
| cfg_aliases | 0.2.2 | MIT |
| codespan-reporting | 0.13.1 | Apache-2.0 |
| crossbeam-channel | 0.5.17 | MIT OR Apache-2.0 |
| crossbeam-utils | 0.8.23 | MIT OR Apache-2.0 |
| crunchy | 0.2.4 | MIT |
| document-features | 0.2.12 | MIT OR Apache-2.0 |
| equivalent | 1.0.3 | Apache-2.0 OR MIT |
| foldhash | 0.2.0 | Zlib |
| futures-core | 0.3.34 | MIT OR Apache-2.0 |
| futures-task | 0.3.34 | MIT OR Apache-2.0 |
| futures-util | 0.3.34 | MIT OR Apache-2.0 |
| gpu-allocator | 0.28.0 | MIT OR Apache-2.0 |
| half | 2.7.1 | MIT OR Apache-2.0 |
| hashbrown | 0.16.1 | MIT OR Apache-2.0 |
| hashbrown | 0.17.1 | MIT OR Apache-2.0 |
| indexmap | 2.14.2 | Apache-2.0 OR MIT |
| itoa | 1.0.18 | MIT OR Apache-2.0 |
| js-sys | 0.3.105 | MIT OR Apache-2.0 |
| libc | 0.2.191 | MIT OR Apache-2.0 |
| libloading | 0.8.9 | ISC |
| libm | 0.2.16 | MIT |
| litrs | 1.0.1 | MIT OR Apache-2.0 |
| lock_api | 0.4.14 | MIT OR Apache-2.0 |
| log | 0.4.35 | MIT OR Apache-2.0 |
| memchr | 2.8.3 | Unlicense OR MIT |
| naga | 30.0.1 | MIT OR Apache-2.0 |
| naga-types | 30.0.1 | MIT OR Apache-2.0 |
| num-traits | 0.2.19 | MIT OR Apache-2.0 |
| once_cell | 1.21.4 | MIT OR Apache-2.0 |
| ordered-float | 5.5.0 | MIT |
| parking_lot | 0.12.5 | MIT OR Apache-2.0 |
| parking_lot_core | 0.9.12 | MIT OR Apache-2.0 |
| pin-project-lite | 0.2.17 | Apache-2.0 OR MIT |
| poc-canvas | 0.1.0 | MIT OR Apache-2.0 |
| poc-common | 0.1.0 | MIT OR Apache-2.0 |
| poc-observer | 0.1.0 | MIT OR Apache-2.0 |
| pollster | 0.4.0 | Apache-2.0/MIT |
| portable-atomic | 1.15.0 | Apache-2.0 OR MIT |
| portable-atomic-util | 0.2.8 | Apache-2.0 OR MIT |
| presser | 0.3.1 | MIT OR Apache-2.0 |
| proc-macro2 | 1.0.107 | MIT OR Apache-2.0 |
| profiling | 1.0.18 | MIT OR Apache-2.0 |
| quote | 1.0.47 | MIT OR Apache-2.0 |
| range-alloc | 0.1.5 | MIT OR Apache-2.0 |
| raw-window-handle | 0.6.2 | MIT OR Apache-2.0 OR Zlib |
| redox_syscall | 0.5.18 | MIT |
| renderdoc-sys | 1.1.0 | MIT OR Apache-2.0 |
| rustc-hash | 1.1.0 | Apache-2.0/MIT |
| rustversion | 1.0.23 | MIT OR Apache-2.0 |
| scopeguard | 1.2.0 | MIT OR Apache-2.0 |
| serde | 1.0.229 | MIT OR Apache-2.0 |
| serde_core | 1.0.229 | MIT OR Apache-2.0 |
| serde_derive | 1.0.229 | MIT OR Apache-2.0 |
| serde_json | 1.0.154 | MIT OR Apache-2.0 |
| slab | 0.4.12 | MIT |
| smallvec | 1.16.4 | MIT OR Apache-2.0 |
| static_assertions | 1.1.0 | MIT OR Apache-2.0 |
| syn | 2.0.119 | MIT OR Apache-2.0 |
| syn | 3.0.9 | MIT OR Apache-2.0 |
| thiserror | 2.0.21 | MIT OR Apache-2.0 |
| thiserror-impl | 2.0.21 | MIT OR Apache-2.0 |
| unicode-ident | 1.0.27 | (MIT OR Apache-2.0) AND Unicode-3.0 |
| unicode-width | 0.2.2 | MIT OR Apache-2.0 |
| wasm-bindgen | 0.2.128 | MIT OR Apache-2.0 |
| wasm-bindgen-futures | 0.4.78 | MIT OR Apache-2.0 |
| wasm-bindgen-macro | 0.2.128 | MIT OR Apache-2.0 |
| wasm-bindgen-macro-support | 0.2.128 | MIT OR Apache-2.0 |
| wasm-bindgen-shared | 0.2.128 | MIT OR Apache-2.0 |
| web-sys | 0.3.105 | MIT OR Apache-2.0 |
| wgpu | 30.0.1 | MIT OR Apache-2.0 |
| wgpu-core | 30.0.1 | MIT OR Apache-2.0 |
| wgpu-core-deps-windows-linux-android | 30.0.1 | MIT OR Apache-2.0 |
| wgpu-hal | 30.0.1 | MIT OR Apache-2.0 |
| wgpu-naga-bridge | 30.0.1 | MIT OR Apache-2.0 |
| wgpu-types | 30.0.1 | MIT OR Apache-2.0 |
| windows | 0.62.2 | MIT OR Apache-2.0 |
| windows-collections | 0.3.2 | MIT OR Apache-2.0 |
| windows-core | 0.62.2 | MIT OR Apache-2.0 |
| windows-future | 0.3.2 | MIT OR Apache-2.0 |
| windows-implement | 0.60.2 | MIT OR Apache-2.0 |
| windows-interface | 0.59.3 | MIT OR Apache-2.0 |
| windows-link | 0.2.1 | MIT OR Apache-2.0 |
| windows-numerics | 0.3.1 | MIT OR Apache-2.0 |
| windows-result | 0.4.1 | MIT OR Apache-2.0 |
| windows-strings | 0.5.1 | MIT OR Apache-2.0 |
| windows-threading | 0.2.1 | MIT OR Apache-2.0 |
| zerocopy | 0.8.63 | BSD-2-Clause OR Apache-2.0 OR MIT |
| zerocopy-derive | 0.8.63 | BSD-2-Clause OR Apache-2.0 OR MIT |
| zmij | 1.0.23 | MIT |

[PresentMon 2.3.0](https://github.com/GameTechDev/PresentMon/releases/tag/v2.3.0) is a separately downloaded Intel tool under the MIT licence. Its source URL and SHA-256 are recorded alongside the executable. No downloaded binary or raw evidence is committed. The generic eye asset comes from `apps/layer-web/icons/layer-eye-symbolic.svg` under MIT OR Apache-2.0.
