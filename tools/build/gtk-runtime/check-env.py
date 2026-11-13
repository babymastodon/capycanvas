#!/usr/bin/env python3
"""Check the native SDK before compiling the private runtime."""
import os
import shlex
import shutil
import subprocess
import sys

compiler = shlex.split(os.environ.get('CC', 'cc'))
cxx = shlex.split(os.environ.get('CXX', 'c++'))
tools = [compiler[0], cxx[0], 'pkg-config', 'meson', 'ninja', 'patchelf',
         'glslc', 'wayland-scanner', 'glib-compile-resources', 'curl', 'patch', 'tar', 'xz']
missing = [tool for tool in tools if not shutil.which(tool)]
if missing:
    sys.exit('Missing native build tools: ' + ', '.join(missing)
             + '\nInstall the Linux build prerequisites before rebuilding; see docs/development/linux.md#prerequisites.')

meson_version = subprocess.check_output(['meson', '--version'], text=True).strip()
if tuple(map(int, meson_version.split('.')[:2])) < (1, 8):
    sys.exit('Meson >= 1.8 is required; found ' + meson_version
             + '\nInstall the Linux build prerequisites before rebuilding; see docs/development/linux.md#prerequisites.')

packages = ['gtk4 >= 4.22', 'libadwaita-1 >= 1.9', 'vulkan >= 1.3',
            'libffi >= 3.0.0', 'mount >= 2.23', 'libpcre2-8 >= 10.32',
            'cairo >= 1.18.2', 'cairo-gobject >= 1.18.2', 'cairo-ft >= 1.18.2',
            'fontconfig >= 2.17.0', 'freetype2', 'harfbuzz >= 11.0.0',
            'harfbuzz-subset >= 8.4.0', 'fribidi >= 1.0.6', 'libthai >= 0.1.9',
            'gdk-pixbuf-2.0 >= 2.30.0', 'graphene-gobject-1.0 >= 1.10.0',
            'epoxy >= 1.4', 'xkbcommon >= 0.2.0', 'libpng', 'zlib',
            'wayland-client >= 1.24.0', 'wayland-scanner >= 1.24.0',
            'wayland-egl', 'wayland-protocols >= 1.48', 'libdrm',
            'cairo-xlib >= 1.18.2', 'x11', 'xext', 'xi', 'xrandr >= 1.2.99', 'xrender',
            'xcursor', 'xdamage', 'xfixes', 'xinerama']
missing = [package for package in packages
           if subprocess.run(['pkg-config', '--exists', package]).returncode]
if missing:
    sys.exit('Missing or outdated native development packages: ' + ', '.join(missing)
             + '\nInstall the Linux build prerequisites before rebuilding; see docs/development/linux.md#prerequisites.')
wayland_version = subprocess.check_output(['pkg-config', '--modversion', 'wayland-client'], text=True).strip()
scanner = subprocess.check_output(['pkg-config', '--variable=wayland_scanner', 'wayland-scanner'], text=True).strip() or 'wayland-scanner'
scanner_result = subprocess.run([scanner, '--version'], check=True, capture_output=True, text=True)
scanner_version = (scanner_result.stdout + scanner_result.stderr).split()[-1]
if scanner_version != wayland_version:
    sys.exit('Wayland scanner ' + scanner_version + ' must match client ' + wayland_version
             + '\nInstall the Linux build prerequisites from one coherent distribution SDK; see docs/development/linux.md#prerequisites.')
flags = shlex.split(subprocess.check_output(['pkg-config', '--cflags', *packages], text=True))
source = '''#define PCRE2_CODE_UNIT_WIDTH 8
#include <gtk/gtk.h>
#include <adwaita.h>
#include <vulkan/vulkan.h>
#include <glib.h>
#include <gio/gio.h>
#include <ffi.h>
#include <libmount/libmount.h>
#include <pcre2.h>
'''
compiler += shlex.split(os.environ.get('CPPFLAGS', '')) + shlex.split(os.environ.get('CFLAGS', ''))
result = subprocess.run([*compiler, *flags, '-x', 'c', '-fsyntax-only', '-'], input=source, text=True)
if result.returncode:
    sys.exit('Native development headers are incomplete. Install the Linux build prerequisites before rebuilding.')
subprocess.run(['glib-compile-resources', '--version'], check=True, stdout=subprocess.DEVNULL)
