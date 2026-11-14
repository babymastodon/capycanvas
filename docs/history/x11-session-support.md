# X11 session support

[Design history](README.md) · [GTK and Wayland canvas subsurface](wayland-subsurface-feasibility.md)

> Research record, November 2026: what running the Linux client in an X11
> session would take, prompted by
> [issue #24](https://github.com/babymastodon/capycanvas/issues/24). It reflects
> GTK 4.24.1 and the client when written; no X11 code exists. Check the code
> before relying on it.

## Summary

X11 support is possible but large. The client shows its canvas through a Wayland
subsurface beneath GTK's transparent controls, and X11 has no equivalent. To keep
the current UI on X11, the client would need:

- a second presentation path that hands each canvas frame to GTK as a texture;
- a patched GTK that reports pens under common tablet drivers;
- acceptance that several display and input features remain Wayland-only.

GTK deprecated its X11 backend in 4.17.4, and the `gdk_x11_*` API has been
deprecated since 4.18. That work would build on a backend upstream is winding
down.

## Why the client does not start on X11

| Gate | Where |
| --- | --- |
| The bundled GTK is built without the X11 backend, so the launcher and packages can't open an X11 display. | `-Dx11-backend=false` in [`build.sh`](../../tools/build/gtk-runtime/build.sh) |
| The Flatpak grants only the Wayland socket. | `--socket=wayland` in [`packaging/flatpak/build.sh`](../../packaging/flatpak/build.sh) |
| With a system GTK that includes X11, the canvas refuses any window that is not a Wayland toplevel. | `Parent::new` in [`wayland.rs`](../../apps/layer-linux/src/wayland.rs) |
| Canvas presentation, frame pacing, color description, the window surround and compositor GPU discovery all use Wayland protocol objects. | [`apps/layer-linux/src/wayland/`](../../apps/layer-linux/src/wayland) |

## Presenting the canvas

The canvas fills the window. The title bar is transparent over it, and panels,
glass, tool bubbles and the canvas action bar sit on top
([title bar](../ui/window-bar.md), [panel transparency](../ui/panel-transparency.md)).
On Wayland, a subsurface below GTK's surface provides this. X11 offers three
candidates:

| Approach | Result |
| --- | --- |
| A child X window for the canvas | An X child window always draws above its parent's content, so every control over the canvas would be hidden. Rejected. |
| A separate toplevel beneath a transparent GTK window | Needs a compositing manager, can't move or resize together with GTK's window, and depends on how each window manager stacks windows. Rejected. |
| Canvas frames as GTK textures | The presenter renders into exportable Vulkan images. Each is wrapped as a `GdkDmabufTexture` and drawn by GTK beneath its controls. This keeps the UI and is the only path that does. |

The texture path is the design the [subsurface record](wayland-subsurface-feasibility.md)
replaced, and it brings back the costs that design avoided:

- **Main-thread dependence.** Each canvas frame goes through GTK's main thread
  and frame clock. It reaches the screen at least one frame later than through
  the subsurface, and a UI-thread stall also stalls the canvas.
- **OpenGL import.** On X11, GTK renders with OpenGL by default; its Vulkan
  default has applied only to Wayland since 4.16. GTK imports dmabufs through
  EGL. If it falls back to GLX, its OpenGL renderer can't import them and GTK
  copies them through the CPU, which is too slow for a canvas.
- **No fence.** `GdkDmabufTextureBuilder` takes no fence. The render worker must
  wait for each frame to finish, or attach a sync file to the buffer, before
  publishing the texture.

## Components that would change

| Component | Change | Size |
| --- | --- | --- |
| X11 canvas path in [`render_thread.rs`](../../apps/layer-linux/src/render_thread.rs) | Add a pool of exportable images created through wgpu-hal with `VK_EXT_external_memory_dma_buf` and `VK_EXT_image_drm_format_modifier`, using the formats from `gdk_display_get_dmabuf_formats`. It also covers completion sync, handoff to the UI thread, reuse after GTK releases a texture, resize, GPU recovery, and damage through `set_update_texture`. | Large; highest risk |
| [`wayland.rs`](../../apps/layer-linux/src/wayland.rs) and its submodules | Split into one canvas-target interface with Wayland and X11 implementations. Geometry and launch completion become shared. The X11 implementation has no equivalent of [`backdrop.rs`](../../apps/layer-linux/src/wayland/backdrop.rs); the window keeps its GTK background. | Medium |
| Chrome handoff in `render_thread.rs` | Wayland holds glass and Navigator changes with synchronized subsurface commits. On X11, GTK draws canvas revision N together with chrome revision N, which changes what the chrome-sync tests assert. | Medium |
| `FrameClock` and the stroke pacer | Both learn from `wp_presentation` feedback. On X11, `GdkFrameTimings` carries presentation times only when the window manager sends `_NET_WM_FRAME_TIMINGS`. Without it, frames report a refresh of 0 and GTK clocks at 60 Hz. | Medium |
| `process_gpu` in [`layer-host`](../../crates/layer-host/src/gpu.rs) | It needs a `wgpu::Surface`. X11 needs a variant without a surface that refuses adapters unable to export dmabufs. X11 also reports no compositor GPU, so the canvas adapter has to match the device behind GTK's EGL display, especially on PRIME laptops. | Medium |
| Bundled GTK X11 pen patch | The X11 backend creates a `GdkDeviceTool` only from xf86-input-wacom's `Wacom Serial IDs` property ([`handle_property_change`](https://github.com/GNOME/gtk/blob/4.24.1/gdk/x11/gdkdevicemanager-xi2.c#L1141-L1179)), and its events carry the virtual core pointer ([motion events](https://github.com/GNOME/gtk/blob/4.24.1/gdk/x11/gdkdevicemanager-xi2.c#L1862-L1868)). Under xf86-input-libinput, which serves Huion, XP-Pen and Wacom without the Wacom driver, `pointer_kind` sees a mouse and the client forces pressure to 1. The patch would create pen and eraser tools with axis flags for pen-class devices, so motion history keeps pressure and tilt. | Medium; carried indefinitely |
| [`input.rs`](../../apps/layer-linux/src/input.rs), [`tablet_input.rs`](../../apps/layer-linux/src/tablet_input.rs) | X11 tilt is normalized to the driver's valuator range rather than ±90°, so the `× π/2` conversion needs the device's range. All tablets share one logical pointer, which collapses the per-device flag cache. `pad_buttons` receives nothing, and pens send no proximity events. | Small to medium |
| GTK runtime, packaging and launchers | Enable the X11 backend. Add its dependencies (`x11`, `xext`, `xi`, `xrandr`, `xrender`, `xcursor`, `xdamage`, `xfixes`, `xinerama`) to [`check-env.py`](../../tools/build/gtk-runtime/check-env.py), the [Arch recipe](../../packaging/arch/capycanvas-git/PKGBUILD) and the [Linux guide](../development/linux.md). Add `--socket=fallback-x11 --share=ipc` to the Flatpak. Make the X11 `WM_CLASS`, taken from the program name, match the desktop entry's `StartupWMClass`. | Small |
| Input methods | GTK 4.24 builds in only its simple and Wayland input methods. On X11, IBus needs the external `im-ibus` module, which the bundled runtime's prefix doesn't contain. | Small to medium |
| Settings | The GTK canvas rendering row shows `settings-vulkan-wayland` ("Vulkan · Wayland") from [`settings.rs`](../../crates/layer-ui/src/settings.rs) in every catalog. X11 needs its own label. | Small |
| Native journeys | All journeys run on `mutter --headless --wayland --no-x11` ([`workspace-motion.sh`](../../tools/performance/workspace-motion.sh)). An X11 mode could enable Mutter's Xwayland and run with `GDK_BACKEND=x11`. That tests GDK's X11 backend but not an Xorg server or its drivers. The tablet proxy can't reach Mutter's Xwayland, so pen journeys would need a separate rootful Xwayland or a uinput tablet on Xorg. Journeys that read presented-frame records need the X11 path to produce them, and the `WAYLAND_DISPLAY` guards need a neutral check. | Medium to large |
| Guides | The [Linux guide](../development/linux.md), [platform integration](../platforms/README.md), [panel transparency](../ui/panel-transparency.md), [GPU fallbacks](../development/gpu-fallbacks.md), the [Arch recipe README](../../packaging/arch/README.md) and the [GTK client agent guide](../../apps/layer-linux/AGENTS.md) | Small |

The shared document, engine, presenter and shaders need no X11-specific change
beyond GPU selection and the settings label.

## Features unavailable or reduced on X11

| Feature | On X11 |
| --- | --- |
| HDR, Display P3 presentation and per-monitor color feedback | GDK has no X11 color management. The canvas uses the existing unmanaged sRGB fallback. |
| Fractional scaling | Integer `GDK_SCALE` only, with one scale for every monitor. |
| Tablet pad buttons, rings and strips as Capy shortcuts | The X11 backend creates no pad events. Buttons arrive as pointer buttons, with ring and strip as scrolling, or as keys mapped by the tablet driver. |
| Pen twist, hover distance and slider | GDK maps only pressure, tilt and wheel valuators ([axis labels](https://github.com/GNOME/gtk/blob/4.24.1/gdk/x11/gdkdevicemanager-xi2.c#L205-L225)). Pen identity, including the eraser end and per-pen settings, needs the Wacom X driver or the GTK patch above. |
| Frame rate and latency | Above 60 Hz only when the window manager sends frame timings. The canvas is at least one frame later than on Wayland and follows GTK's main thread. The presented-frame readout falls back to submitted frames. |
| Rounded corners, shadows and translucent popups | These need a compositing manager. Without one, windows have square corners and no shadow, and popups such as the command bar lose their transparency. Glass panels over the canvas still work because the presenter draws the blur into the canvas texture. |

Linux has no performance tier ([performance targets](../PERFORMANCE_TARGETS.md)),
so X11 frame rate would be best effort.

## Unproven on hardware

- **NVIDIA's proprietary driver.** GDK tries EGL with strict checks and falls
  back to GLX, where it can't import dmabufs. Users have reported gaps in
  NVIDIA's EGL dmabuf import
  ([NVIDIA forums](https://forums.developer.nvidia.com/t/status-of-linux-dma-buf-support/209404)). Whether a GeForce on Xorg imports the canvas without a CPU
  copy decides whether the texture path is usable there.
- **Huion's proprietary driver.** GDK classifies a device as a pen only when its
  name contains `pen`, `stylus`, `eraser` or `wacom`. The name of the driver's
  virtual tablet decides whether even a patched GTK sees a pen.
- **Hybrid graphics.** The canvas and GTK's EGL display must share a device, or
  the export must use a format both can read.

## Options

- **Clear refusal.** Build the X11 backend only so the client can explain that it
  needs a Wayland session, instead of exiting because it can't open a display.
  This is small and touches only the runtime build and startup.
- **Wayland sessions.** The client runs on Wayland desktops such as GNOME.
  Linux Mint plans to drop the experimental label from Cinnamon's Wayland
  session in its next Cinnamon release
  ([report](https://omgubuntu.co.uk/2026/07/linux-mint-wayland-fully-supported)).
- **Full X11 support.** All the components above, with the listed features
  remaining Wayland-only.

## Recommendation

Ship the clear refusal. Consider full support only after a spike on an X11
desktop with NVIDIA's proprietary driver and a Huion tablet shows two things:

- GTK imports exported canvas images without a CPU copy at 4K;
- a patched GTK reports pen pressure and tilt from the tablet.

If either fails, X11 support would cost painting quality, not only features.
