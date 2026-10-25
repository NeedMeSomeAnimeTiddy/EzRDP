# Working on EzRDP

## Checking the Windows version (PC app)

Before pushing any change under `server/`, run both:

```sh
cd server && cargo test            # Linux version
server/check-windows.sh            # Windows version: MinGW build + tests under Wine
```

`check-windows.sh` installs MinGW and Wine the first time (about 4 minutes for the first
build, under a minute after that). It catches Windows-only compile errors and runs the
Windows-only tests without needing a push.

Why this way:

- **MinGW + Wine (local)** is fast and works in any Linux session, so mistakes are fixed
  before they reach GitHub.
- **GitHub Actions** (`.github/workflows/build.yml`) builds on a real Windows machine with
  Microsoft's compiler. That is the official `EzRDP.exe`. After pushing, check that the
  "PC app (Windows)" job passed.
- Don't hand out the MinGW `.exe`: it's built with a different compiler and needs extra
  DLLs next to it. It's only for checking.
- Don't try to build for `x86_64-pc-windows-msvc` on Linux: the video encoder's C code needs
  Microsoft's Windows headers, which aren't available there.
