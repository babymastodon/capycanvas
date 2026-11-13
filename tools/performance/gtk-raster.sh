#!/usr/bin/env bash
# Run an already built GTK test on an isolated 120 Hz Wayland display.
# Keeping compilation separate makes before/after measurement reproducible.
set -euo pipefail
raster_script=$(realpath "${BASH_SOURCE[0]}")
if [[ "${1:-}" != --session ]]; then
    exec dbus-run-session -- bash "$raster_script" --session "$@"
fi
shift
if [[ $# != 3 ]]; then
    echo 'Usage: gtk-raster.sh TEST_EXECUTABLE TEST_FILTER REPORT_PREFIX' >&2
    exit 2
fi
raster_binary=$(realpath "$1")
raster_filter=$2
raster_report=$(realpath -m "$3")
raster_run_dir=$(mktemp -d /tmp/capy-gtk-raster.XXXXXX)
mkdir -p "$(dirname "$raster_report")"
export XDG_RUNTIME_DIR="$raster_run_dir/runtime"
mkdir -m 700 "$XDG_RUNTIME_DIR"
export WAYLAND_DISPLAY=layer-bench-raster
export GDK_BACKEND=wayland GSK_RENDERER=vulkan GTK_A11Y=${GTK_A11Y:-none}
# Native file tests drive the in-process GTK chooser. Portal dialogs live in a
# different process and cannot be exercised by those widget signal assertions.
# Match production's GTK color-management opt-in and preserve diagnostic flags.
export GDK_DEBUG="${GDK_DEBUG:+$GDK_DEBUG:}no-portals:color-mgmt"
export XDG_CONFIG_DIRS="$(dirname "$raster_script")/gtk-config:${XDG_CONFIG_DIRS:-/etc/xdg}"
export XDG_DATA_HOME="$raster_run_dir/data"
mkdir "$XDG_DATA_HOME"
export CAPY_STORAGE_DIR="$raster_run_dir/storage"
mkdir -p "$CAPY_STORAGE_DIR/data/workspaces" "$CAPY_STORAGE_DIR/state/sessions"
export LAYER_PACING_REPORT="$raster_report.json"
unset DISPLAY
raster_xwayland=(--no-x11)
[[ ${LAYER_TEST_X11:-} == 1 ]] && raster_xwayland=()
env -u G_DEBUG mutter --headless --wayland "${raster_xwayland[@]}" \
    --virtual-monitor="${LAYER_TEST_MONITOR:-1600x1000@120}" --wayland-display="$WAYLAND_DISPLAY" \
    >"$raster_report-mutter.log" 2>&1 &
raster_compositor_pid=$!
trap 'kill "$raster_compositor_pid" 2>/dev/null || true; wait "$raster_compositor_pid" 2>/dev/null || true' EXIT
for ((attempt=0; attempt<100; attempt++)); do
    [[ -S "$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY" ]] && break
    kill -0 "$raster_compositor_pid" 2>/dev/null || { cat "$raster_report-mutter.log"; exit 1; }
    sleep .1
done
[[ -S "$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY" ]]
if [[ ${LAYER_TEST_SCALE:-1} != 1 ]]; then
    gdbus wait --session --timeout 10 org.gnome.Mutter.DisplayConfig
    gjs "$(dirname "$raster_script")/display-scale.js" "$LAYER_TEST_SCALE"
fi
if [[ ${LAYER_TEST_X11:-} == 1 ]]; then
    for ((attempt=0; attempt<100; attempt++)); do
        LAYER_TEST_X11_DISPLAY=$(sed -n 's/.*Using public X11 display \(:[0-9]*\).*/\1/p' "$raster_report-mutter.log")
        [[ -n $LAYER_TEST_X11_DISPLAY ]] && break
        sleep .1
    done
    [[ -n $LAYER_TEST_X11_DISPLAY ]]
    raster_xauthority=("$XDG_RUNTIME_DIR"/.mutter-Xwaylandauth.*)
    [[ -f ${raster_xauthority[0]} ]]
    export LAYER_TEST_X11_DISPLAY DISPLAY=$LAYER_TEST_X11_DISPLAY XAUTHORITY=${raster_xauthority[0]}
    unset WAYLAND_DISPLAY GDK_BACKEND GSK_RENDERER
fi
cd "$(dirname "$raster_script")/../../apps/layer-linux"
G_DEBUG=fatal-criticals "$raster_binary" "$raster_filter" --ignored --test-threads=1 \
    --nocapture 2>&1 | tee "$raster_report.log"
