#!/usr/bin/env bash
# Probe GPU / capture / encoder facts. Print what exists. Never create
# device nodes, never stub NVENC, never treat a missing GPU as present.
set -u

have() { command -v "$1" >/dev/null 2>&1; }
sec() { printf '\n== %s ==\n' "$1"; }

echo "SplitDesk GPU diagnostics (facts only)"
echo "host: $(uname -s 2>/dev/null || echo unknown) $(uname -m 2>/dev/null || echo unknown)"
echo "date: $(date -u +%Y-%m-%dT%H:%M:%SZ 2>/dev/null || echo unknown)"

sec "nvidia"
if have nvidia-smi; then
  nvidia-smi -L 2>/dev/null || echo "nvidia-smi -L failed"
  nvidia-smi --query-gpu=name,driver_version,memory.total --format=csv,noheader 2>/dev/null || true
else
  echo "nvidia-smi: unavailable"
fi
if [[ -e /dev/nvidia0 || -e /dev/nvidiactl ]]; then
  echo "nvidia device nodes: present"
  ls -l /dev/nvidia* 2>/dev/null || true
else
  echo "nvidia device nodes: absent (not created by this script)"
fi

sec "drm render nodes"
if [[ -d /dev/dri ]]; then
  ls -l /dev/dri 2>/dev/null || true
else
  echo "/dev/dri: absent"
fi

sec "vaapi"
if have vainfo; then
  vainfo 2>/dev/null || echo "vainfo failed"
else
  echo "vainfo: unavailable"
fi

sec "intel / amd sysfs"
if [[ -d /sys/class/drm ]]; then
  ls /sys/class/drm 2>/dev/null || true
else
  echo "/sys/class/drm: absent"
fi

sec "pipewire / wayland"
if have pw-cli; then
  pw-cli info 0 2>/dev/null | head -n 20 || echo "pw-cli info failed"
else
  echo "pw-cli: unavailable"
fi
echo "WAYLAND_DISPLAY=${WAYLAND_DISPLAY:-unset}"
echo "XDG_RUNTIME_DIR=${XDG_RUNTIME_DIR:-unset}"
echo "XDG_SESSION_TYPE=${XDG_SESSION_TYPE:-unset}"

sec "egl / glvnd"
if have eglinfo; then
  eglinfo 2>/dev/null | head -n 40 || echo "eglinfo failed"
else
  echo "eglinfo: unavailable"
fi
if have nvidia-smi && nvidia-smi -L >/dev/null 2>&1; then
  echo "nvenc: probe via encoder binary / splitdesk diagnostics gpu (not assumed here)"
else
  echo "nvenc: unavailable (no nvidia-smi list)"
fi

sec "splitdesk"
if have splitdesk; then
  splitdesk diagnostics gpu 2>/dev/null || echo "splitdesk diagnostics gpu failed"
  splitdesk diagnostics media-path 2>/dev/null || echo "splitdesk diagnostics media-path failed"
else
  echo "splitdesk CLI: not in PATH"
fi

echo
echo "done. Missing devices stay missing; this script does not fake a GPU."
exit 0
