# Nyx demo

You are in a Codespace running Nyx in QEMU. The desktop opens in a new browser tab on port **6080**
as soon as it has booted — usually within a couple of minutes. If the tab did not open, use the
**Ports** panel and open *Nyx desktop*.

Boot progress and the kernel log:

```bash
tail -f /tmp/nyx-demo.log ~/.nyx-demo/serial.log
```

## Things to try

- Click the icons in the dock (bottom-left) — Files, Terminal, QCLang, Notepad, System Monitor,
  Image Viewer, Settings.
- The **Command**: press the Super/Windows key if your browser passes it through. The dock works
  either way.
- In **Terminal**: `help`, `quantum run bell`, `sched`, `gpu`.

## What this is and isn't

This is the same image you could write to a USB stick, running under QEMU. QEMU has no Intel GPU,
so everything is drawn on the CPU; there is no Wi-Fi, no touchpad, and no network in this demo.
The mouse moves at about twice the speed of your pointer (the PS/2 driver doubles motion), so the
two cursors drift apart — the keyboard is the smoother way around.

To run it on your own machine instead: `tools/demo/run-demo.sh` (see the comments at the top for
the packages it needs), then open <http://localhost:6080/>.
