# EzRDP

Small, fast remote desktop: control your Windows PC from your Android phone.

- **PC side:** one small `.exe` written in Rust. No install, no drivers.
- **Phone side:** a lightweight Android app.
- **Your monitors stay on.** EzRDP only *reads* the screen. It never changes display
  settings, never locks the PC and never turns screens off.

## What you get

| | |
|---|---|
| Mouse | Trackpad mode (like a laptop touchpad) or Touch mode (tap where you want) |
| Keyboard | Phone keyboard types on the PC. Extra keys bar: Esc, Tab, Ctrl, Alt, Shift, Win, arrows, F1–F12, Task Manager, Alt+Tab… |
| Screens | Pick which monitor to view |
| Sound | Hear the PC on your phone (toggle on/off) |
| Zoom | Pinch to zoom in on small text |
| Quality | High / Balanced / Data saver |

### Gestures (Trackpad mode)

| Do this | Get this |
|---|---|
| Drag 1 finger | Move pointer |
| Tap | Left click |
| Tap, then tap-and-hold and drag | Click and drag (select text, move windows) |
| 2-finger tap, or long press | Right click |
| 2-finger drag | Scroll |
| Pinch | Zoom in/out |

In **Touch mode** the pointer jumps to your finger: tap = click there, drag = drag, long press = right click.

## Setup

### PC

1. Put `EzRDP.exe` in a folder you'll keep (for example `C:\EzRDP`) and run it.
   If SmartScreen warns you, click **More info → Run anyway** (the file isn't code-signed).
2. Windows asks once to allow changes: click **Yes**. That lets EzRDP through Windows Firewall.
3. EzRDP now lives in the **tray** (the icons next to the clock) and starts with Windows.
   Right-click its icon for:
   - **Pause / Resume connections**: while paused, no phone can connect and anyone connected is dropped.
   - **Trusted phones**: see and remove phones.
   - **Start with Windows**, **Connection info**, **Quit**.

### Phone (one time, at home)

1. Install `EzRDP.apk` (allow "install unknown apps" when asked).
2. Make sure the phone is on the **same Wi-Fi** as the PC and open EzRDP. Your PC shows up by itself.
3. Tap it. The phone shows a 6-digit code and the PC asks **"A phone wants to control this PC"** with
   the same code. Click **Yes**.

That's it. From then on, just tap the PC in the list, at home or away. There are no addresses
or passwords to type: the phone remembers the PC's home and internet addresses and picks the
right one.

### Connecting from outside your home

The PC tries to open its port on your router automatically (UPnP). If that doesn't work, forward
**TCP port 7270** on your router to the PC (Connection info in the tray menu shows its home address).
If your internet provider uses CGNAT (Connection info will say so), outside connections can't reach
your PC at all; use Tailscale in that case.

If your home internet address changes, the phone learns the new one the next time it connects at
home. People with a DDNS name can set it per PC: long-press the PC in the app → Set internet address.

## Security

- Pairing only works from the home network, and only after you click Yes on the PC.
- Pairing uses P-256 key exchange with a commitment and a 6-digit comparison code (like Bluetooth),
  so nobody can sneak in between the phone and the PC while pairing.
- Every connection logs in with the phone's own key and is encrypted with AES-128-GCM.
- Remove a phone from the tray menu and it can't connect any more.

## Limits

- The PC must be signed in. The Windows lock screen and UAC prompts can't be shown (Windows blocks that for normal apps). The picture comes back when they're gone.
- One phone at a time. A new connection takes over from the old one.
- EzRDP runs as a tray app, not a Windows service: services can't see your screen.

## How it works (short)

- Screen capture: DXGI Desktop Duplication (GPU, read-only, only sends frames when something changes).
- Video: H.264 (OpenH264, screen-content mode) → hardware decoder on the phone, with low-latency mode.
- Lag control: the PC only sends a new frame once the phone has confirmed the earlier ones, so delay can't pile up on slow connections.
- Input: Windows `SendInput`. Sound: WASAPI loopback → raw 16-bit PCM.

## Building yourself

- PC: install Rust, then `cd server && cargo build --release`
- Android: `cd android && ./gradlew assembleRelease` (needs the Android SDK)
