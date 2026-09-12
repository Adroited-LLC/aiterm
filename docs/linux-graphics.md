# NVIDIA Wayland compatibility

Settings → Diagnostics → Graphics compatibility offers **Automatic**, **On**, and
**Off**. New settings default to Automatic. Both Automatic and On apply
`__NV_DISABLE_EXPLICIT_SYNC=1` before GTK initializes, only on Linux when the
NVIDIA kernel module is loaded and the launch environment indicates Wayland.
Off disables AiTerm's injection. There is no additional launcher, crash retry,
watchdog, or background graphics polling.

This is NVIDIA's documented workaround for EGL explicit-sync compatibility
failures. It does not disable DMA-BUF rendering. Disabling explicit sync can
reduce performance or cause out-of-order frames on some systems; Off remains
available for users who do not need the workaround. Detecting the NVIDIA module
is a conservative hardware check, not proof that a hybrid system is rendering
AiTerm on its NVIDIA GPU. Hardware validation is still needed on affected machines.

The preference is saved in `graphics.json` in AiTerm's application data directory
(normally `~/.local/share/aiterm/graphics.json` on Linux):

```json
{ "mode": "automatic" }
```

Other values are `"on"` and `"off"`. Existing files with
`"nvidia_wayland_compatibility": true` or `false` retain their explicit On or Off
choice. A missing file defaults to Automatic; unreadable or malformed settings
fall back to Off. Changing the selection writes the new format atomically.

AiTerm reads it before initializing GTK. Changing the selection saves the
preference for the next launch; it never changes the running graphics stack or
restarts the app. Diagnostics reports the saved mode, whether the environment
matches, whether the workaround is active for this run, and whether the saved
choice differs from the startup choice.

An existing `__NV_DISABLE_EXPLICIT_SYNC` environment variable always wins,
including `0` or an empty value. Diagnostics identifies this override so the saved
selection is not mistaken for the active setting. An override supplied by the
user is preserved in terminal children. AiTerm removes only its own injected
value from every new PTY, including cloned terminal managers.

Backend detection handles ordered `GDK_BACKEND` lists. `wayland,x11` permits the
workaround; a leading `x11` with a nonempty `DISPLAY` does not. If X11 is not
advertised, detection considers a later Wayland entry. Wildcards and the unset
backend use the advertised Wayland display/socket. This is startup environment
detection, not a live probe of compositor connectivity.

If an affected machine previously saved Off, launch with
`__NV_DISABLE_EXPLICIT_SYNC=1 aiterm`, select Automatic or On, then launch normally.
The control is hidden in the Windows/WSL frontend.

Tests cover backend ordering, automatic detection and explicit opt-out,
platform/GPU guards, environment overrides, legacy preference migration,
persistence and write failures, restart indication, and PTY environment
inheritance. No NVIDIA hardware is available on the development host.

References: [NVIDIA's workaround](https://github.com/NVIDIA/egl-wayland2#known-issues-and-workarounds)
and [GTK backend selection](https://gnome.pages.gitlab.gnome.org/gtk/gtk3/running.html).
