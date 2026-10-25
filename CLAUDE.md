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

## Checking the phone app

Before pushing any change under `android/`, build it and run its tests. Without an Android
SDK in the session, install one first (about 5 minutes):

```sh
export ANDROID_HOME=$HOME/android-sdk
mkdir -p $ANDROID_HOME && cd $ANDROID_HOME
curl -sSfL -o tools.zip https://dl.google.com/android/repository/commandlinetools-linux-11076708_latest.zip
unzip -q tools.zip && mkdir -p cmdline-tools/latest && mv cmdline-tools/{bin,lib,NOTICE.txt,source.properties} cmdline-tools/latest/
yes | cmdline-tools/latest/bin/sdkmanager --licenses >/dev/null
cmdline-tools/latest/bin/sdkmanager "platforms;android-35" "build-tools;35.0.0" "platform-tools"
```

Then, from `android/`: `./gradlew --no-daemon assembleDebug testDebugUnitTest`.
The tests re-save `screenshots/*.png`; put those back (`git checkout screenshots/`) unless the
change is meant to alter the screens. The official `EzRDP.apk` comes from GitHub Actions.
