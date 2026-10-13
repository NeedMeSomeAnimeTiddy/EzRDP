# EzRDP

Small, fast remote desktop: control your Windows or Linux PC from your Android phone,
or from another Windows or Linux PC.

- **PC side:** one small program written in Rust (`EzRDP.exe` on Windows, `ezrdp-linux` on Linux). No install, no drivers.
  The same program lets other devices control this PC *and* lets you control other PCs from it.
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
banner at the top ("… is controlling this PC"). Click the banner, or press **Ctrl+Esc** on the
PC's keyboard, to disconnect the phone. (Ctrl+Esc sent from the phone still opens Start as usual.)
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

## PC to PC

Control one PC from another. Any mix works: Windows → Windows, Windows → Linux, Linux → Windows, Linux → Linux.

1. Run EzRDP on both PCs: `EzRDP.exe` on Windows (see Setup below), `ezrdp-linux` on Linux (see "Linux" below).
2. On the PC **you sit at**, open the EzRDP window: it opens when you start EzRDP. On Windows you can also
   right-click the tray icon → **Control another PC...**
3. Both on the same home network: the other PC shows up under **Other PCs on this network**. Click **Pair**.
   The other PC asks to allow it and shows a 6-digit code. Check it matches the code on your screen, then click **Yes**.
   (Not showing up? Type its home address under **Pair by address**.)
4. From then on click **Connect** next to it, at home or away (same rules as the phone, see below).

In the window:

| | |
|---|---|
| Mouse | Moves, clicks and scrolls on the other PC wherever you point at the picture |
| Keyboard | Typing goes straight to the other PC, in any language. Shortcuts like Ctrl+C work |
| **Keys** menu | Keys your own PC would grab: Windows key, Alt+Tab, Alt+F4, Task Manager, Lock, Print Screen. Also **Type the text copied on this computer** |
| Screen list | Pick the other PC's monitor (shown when it has more than one) |
| Quality, Sound | Same as on the phone |
| **Full screen** | Fills your screen. Move the mouse to the top edge to get the bar back |

If the connection drops, the window reconnects by itself for 30 seconds.

Two PCs can control each other at the same time. While a PC plays the other one's sound, it doesn't send its
own sound back (that would echo forever).

### Linux

1. Download `ezrdp-linux`, make it runnable and start it from your desktop:
   `chmod +x ezrdp-linux && ./ezrdp-linux`
   EzRDP puts an icon in the panel (next to the clock) and opens its window. While it runs, your phone
   or other PCs can control this PC. Closing the window keeps that going.
2. The icon's menu: **Control another PC...** (or just click the icon), **Pause connections**,
   **Trusted devices**, **Connection info**, **Start when I sign in**, **Quit EzRDP**.
   Starting EzRDP again while it runs just opens the window.
3. If you use a firewall, let port 7270 in: `sudo ufw allow 7270`.

Notes:
- Works on X11 (Xorg) and Wayland desktops. On Wayland (GNOME, KDE...) the PC asks once, the first time
  someone connects, whether to allow screen sharing and remote control; click **Share/Allow** there. It's
  remembered after that. Desktops without remote control support (Sway, Hyprland) share the picture only.
- Pairing asks with a desktop dialog if `zenity` or `kdialog` is installed, otherwise in the terminal.
- Sound needs `parec` (package `pulseaudio-utils`; works with PipeWire too).
- On plain GNOME (e.g. Fedora) the panel icon needs the "AppIndicator and KStatusNotifierItem Support"
  extension; Ubuntu, KDE, Cinnamon, XFCE and MATE show it as is.
- Settings, trusted devices and the log are in `~/.config/ezrdp/`. Delete a line in `trusted.txt` to remove a device.
- Needs OpenGL, ALSA, PipeWire and `libxkbcommon-x11`, which normal desktops already have.

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
   - **Control admin windows too**: Windows blocks the mouse and keyboard from normal programs
     while a program running as administrator is in front (Task Manager, installers). Turn this on
     (Windows asks to allow it) and EzRDP runs, and starts with Windows, as administrator, so the
     phone can use those too. The lock screen and "allow changes?" prompts still can't be controlled.
   - **Control another PC...**: opens the EzRDP window (it also opens when you start EzRDP yourself).

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
- Linux PC: no graphics-card video encoding, no "being controlled" banner,
  and it can't show a phone's screen. The screen saver can still lock the PC while it's being controlled.
- When controlling another PC you see your own mouse pointer; the other PC's pointer shape (text cursor, hand...) isn't shown.

## How it works (short)

- Screen capture: DXGI Desktop Duplication (GPU, read-only, only sends frames when something changes).
- Video: H.264, encoded on the PC's graphics card through Windows Media Foundation (NVIDIA, AMD or Intel),
  or OpenH264 in software if that isn't available → hardware decoder on the phone, low-latency mode,
  drawn straight to the screen (SurfaceView).
- Bitrate adapts: it drops when the network can't keep up and climbs back when it can.
- Lag control: the PC only sends a new frame once the phone has confirmed the earlier ones, so delay can't pile up on slow connections.
- Input: Windows `SendInput`. Sound: WASAPI loopback → raw 16-bit PCM.
- Linux PC: X11 shared-memory screen capture and XTEST for mouse and keyboard, or on Wayland the desktop's
  remote desktop portal (picture through PipeWire), `parec` for sound, OpenH264 video.
- Controlling another PC: OpenH264 decoding, egui window, keyboard sent as typed text (so any layout works) plus real key presses for shortcuts.

## Building yourself

- PC: install Rust, then `cd server && cargo build --release` (Windows or Linux; on Linux install
  `libasound2-dev libpipewire-0.3-dev clang` first)
- Android: `cd android && ./gradlew assembleRelease` (needs the Android SDK). Builds are signed with
  `android/app/ezrdp.keystore` so every new APK installs as an update over the last one.
