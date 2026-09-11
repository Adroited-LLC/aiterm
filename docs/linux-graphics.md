# NVIDIA Wayland compatibility

Settings → Diagnostics → Graphics compatibility contains the **NVIDIA Wayland
compatibility** switch. It defaults off. When explicitly enabled, AiTerm injects
`__NV_DISABLE_EXPLICIT_SYNC=1` only on Linux when the NVIDIA kernel module is
loaded and the launch environment indicates Wayland. This works around NVIDIA
EGL explicit-sync compatibility failures without disabling DMA-BUF rendering.
It is a compatibility workaround, not a general rendering optimization.

The preference is saved in `graphics.json` in AiTerm's application data directory
(normally `~/.local/share/aiterm/graphics.json` on Linux):

```json
{ "nvidia_wayland_compatibility": false }
```

AiTerm reads it before initializing GTK. Changing the switch saves the preference
for the next launch; it never changes the running graphics stack or restarts the
app. Diagnostics separately reports whether the workaround was injected for this
run, and whether the saved choice differs from the startup choice.

An existing `__NV_DISABLE_EXPLICIT_SYNC` environment variable always wins,
including `0` or an empty value. Diagnostics identifies this override so the saved
switch is not mistaken for the active setting. An override supplied by the user
is preserved in terminal children. AiTerm removes only its own injected value
from every new PTY, including cloned terminal managers.

Backend detection handles ordered `GDK_BACKEND` lists. `wayland,x11` permits the
workaround; a leading `x11` with a nonempty `DISPLAY` does not. If X11 is not
advertised, detection considers a later Wayland entry. Wildcards and the unset
backend use the advertised Wayland display/socket. This is startup environment
detection, not a live probe of compositor connectivity.

To reach the UI on an affected machine even if the saved preference is off, launch
`__NV_DISABLE_EXPLICIT_SYNC=1 aiterm`, enable the setting, then launch normally.
The control is hidden in the Windows/WSL frontend.

Tests cover backend ordering, opt-out and platform/GPU guards, environment
overrides, preference persistence and write failures, restart indication, and
PTY environment inheritance. Hardware validation of native NVIDIA rendering is
still required on an affected machine.

References: [NVIDIA's workaround](https://github.com/NVIDIA/egl-wayland2#known-issues-and-workarounds)
and [GTK backend selection](https://gnome.pages.gitlab.gnome.org/gtk/gtk3/running.html).
