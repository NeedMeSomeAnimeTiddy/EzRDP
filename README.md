# EzRDP

Small, fast remote desktop: control your Windows PC from your Android phone.

- **PC side:** one small `.exe` written in Rust. No install, no drivers.
- **Phone side:** a lightweight Android app.
- **Your monitors stay on.** EzRDP only *reads* the screen. It never changes display
  settings, never locks the PC and never turns screens off.

## What you get

| | |
|---|---|
| Mouse | **Both** mode (default): tap to click right there, drag to move the pointer like a trackpad. Also plain Trackpad or Touch mode |
| Keyboard | Phone keyboard types on the PC. Extra keys bar: Esc, Tab, Ctrl, Alt, Shift, Win, arrows, F1–F12, Task Manager, Alt+Tab… |
| Screens | Pick which monitor to view |
| Sound | Hear the PC on your phone (toggle on/off) |
| Zoom | Pinch to zoom in on small text |
| Quality | High / Balanced / Data saver. Uses the PC's graphics card to encode when it can (less delay) |
| Phone → PC | Let the PC control your phone too: hold a PC in the list → **Let this PC control my phone** |

### On-screen buttons

Game-style buttons sit over the picture (move them with ☰ → **Edit layout**):

| Button | What it does |
|---|---|
| **L / M / R** | Left, middle, right mouse button. Hold one and drag with another finger to drag things. |
| **▲ / ▼** | Scroll one step; hold to keep scrolling. |
| **≡** (between them) | Drag up/down like a mouse wheel. |
| **+ / − / Fit** | Zoom in/out around the pointer (hold to keep zooming), or back to the whole screen. While zoomed, push the pointer to an edge and the view glides that way. |
| **⌨** | Phone keyboard (the PC picture shrinks to fit above it). |
| **Screen** | Switch to the PC's next monitor. |
| **Hold** | Keeps the left button pressed until you tap it again (easy dragging). |
| **Hide / Show** | Hides the buttons for a clean view. |

**Settings** (on the main screen, or ☰ → Settings): control mode, pointer speed and size, tap to
click, scroll speed and direction, button size and visibility, left-handed layout, vibration,
quality and sound.

The phone turns sideways when you connect (Settings → Screen to turn that off).
Upright and sideways screens each have their own button layout.

While a phone is connected, the PC shows a blue frame around the screen being viewed and a
banner at the top ("… is controlling this PC"). Click the banner to disconnect the phone.
Windows 10 (2004+) and 11 keep the frame and banner out of what the phone sees.

### Gestures (Both mode)

| Do this | Get this |
|---|---|
| Tap | Pointer jumps there and left-clicks |
| Drag 1 finger | Move pointer like a trackpad |
| Drag starting on the pointer | Pointer follows your finger exactly |
| Tap, then tap-and-hold and drag | Click and drag (select text, move windows) |
| Long press | Right click there |
| 2-finger tap | Right click |
| 2-finger drag | Scroll |
| Pinch | Zoom in/out |

**Trackpad mode:** taps click where the pointer is. **Touch mode:** the pointer is always under your finger.

### Letting the PC control your phone

On the phone, hold your PC in the list → **Let this PC control my phone**. The first time, the app
walks you through switching on "EzRDP" in Android's Accessibility settings (that's what lets the PC
tap and type). Then Android asks to share the screen. A window showing the phone opens on the PC:

| On the PC | On the phone |
|---|---|
| Click / drag | Tap / swipe |
| Hold the mouse button | Long press |
| Right-click, Esc | Back |
| Middle-click | Home |
| Mouse wheel | Scroll |
| Typing, Backspace, Enter | Typed into the selected text box |
| Menu: Recent apps, Notifications | Same |

Stop from the phone's notification, or close the window on the PC.

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
- Clicks and typing always go to the screen the phone is showing (the mouse is moved there first).
- EzRDP runs as a tray app, not a Windows service: services can't see your screen.

## How it works (short)

- Screen capture: DXGI Desktop Duplication (GPU, read-only, only sends frames when something changes).
- Video: H.264, encoded on the PC's graphics card through Windows Media Foundation (NVIDIA, AMD or Intel),
  or OpenH264 in software if that isn't available → hardware decoder on the phone, low-latency mode,
  drawn straight to the screen (SurfaceView).
- Bitrate adapts: it drops when the network can't keep up and climbs back when it can.
- Lag control: the PC only sends a new frame once the phone has confirmed the earlier ones, so delay can't pile up on slow connections.
- Input: Windows `SendInput`. Sound: WASAPI loopback → raw 16-bit PCM.

## Building yourself

- PC: install Rust, then `cd server && cargo build --release`
- Android: `cd android && ./gradlew assembleRelease` (needs the Android SDK). Builds are signed with
  `android/app/ezrdp.keystore` so every new APK installs as an update over the last one.
