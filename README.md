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

### 1. Get the files

Open the **Actions** tab on GitHub, open the newest green **Build** run, and download:
- `EzRDP-PC` → contains `ezrdp-server.exe`
- `EzRDP-Android` → contains `EzRDP.apk`

(If you push a tag like `v1.0`, both files also show up on the **Releases** page.)

### 2. PC

1. Put `ezrdp-server.exe` in any folder (for example `C:\EzRDP`) and run it.
2. If Windows asks about the firewall, click **Allow** and tick **both Private and Public**
   (needed for connecting from outside your home).
   If SmartScreen warns you, click **More info → Run anyway** (the file isn't code-signed).
3. The window shows your **Address**, **Port** and **Password**. Settings live in `ezrdp.conf` next to the exe.

To start it with Windows: press `Win+R`, type `shell:startup`, and put a shortcut to `ezrdp-server.exe` in that folder.

### 3. Phone

1. Install `EzRDP.apk` (allow "install unknown apps" for your browser/files app when asked).
2. Enter the **Address** and **Password** from the PC window. Tap **Connect**.
3. Tap the **☰** button at the top for the toolbar.

That's it. It works on home Wi-Fi and on mobile data. At home the app automatically switches
to the faster direct connection.

### How "connect from anywhere" works

- On start, EzRDP asks your router to open its port (UPnP). Most home routers allow this by default.
- Everything is encrypted (AES-GCM) with a key made from your password. The password itself is never sent.
- If the PC window says it **couldn't open to the internet**:
  - "no UPnP router found" / "router refused": turn on UPnP in your router settings, or
    forward **TCP port 7270** to this PC by hand.
  - "your internet provider shares one address": your provider uses CGNAT, so nothing from the
    internet can reach your PC directly. Ask them for a public IP, or use Tailscale instead.
- Your home internet address can change now and then (most providers). If the phone stops
  connecting from outside, check the Address in the PC window and update it in the app.
- Don't want it reachable from outside? Set `internet=off` in `ezrdp.conf`.

## Limits

- The PC must be signed in. The Windows lock screen and UAC prompts can't be shown (Windows blocks that for normal apps). The picture comes back when they're gone.
- One phone at a time. A new connection takes over from the old one.

## How it works (short)

- Screen capture: DXGI Desktop Duplication (GPU, read-only, only sends frames when something changes).
- Video: H.264 (OpenH264, screen-content mode) → hardware decoder on the phone, with low-latency mode.
- Lag control: the PC only sends a new frame once the phone has confirmed the earlier ones, so delay can't pile up on slow connections.
- Input: Windows `SendInput`. Sound: WASAPI loopback → raw 16-bit PCM.
- Security: PBKDF2 password key, HMAC challenge login, AES-128-GCM on every message.

## Building yourself

- PC: install Rust, then `cd server && cargo build --release`
- Android: `cd android && ./gradlew assembleRelease` (needs the Android SDK)
