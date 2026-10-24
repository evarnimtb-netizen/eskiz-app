# Eskiz packaging

How to build the release packages and regenerate the icon. Everything here is run from the
`eskiz/` directory unless a path says otherwise. Release builds are made by
`.github/workflows/release.yml` when a `v*` tag is pushed. The tag must match the `version`
in the root `Cargo.toml`, which is checked first.

```
packaging/
  icon-gen/        Rust program that draws the icon in code and writes the icon set
  icons/           generated: png/eskiz-<N>.png (16 to 1024), eskiz.icns, eskiz.ico
  macos/           build.sh, Info.plist.in  -> dist/release/Eskiz-<v>-macos-universal.dmg
  windows/         build.ps1                -> dist/release/Eskiz-<v>-windows-x64.zip
  linux/           eskiz.desktop, eskiz.xml (shared MIME), install.sh, build-appimage.sh
                                            -> Eskiz-<v>-x86_64.AppImage
                                               Eskiz-<v>-linux-x86_64.tar.gz
```

Packages are written to `dist/release/`. `dist/` also holds the older cross-build output
from `build_all.sh`; the packaging scripts only replace their own files.

## Icon

The icon is the Eskiz v5 artwork: a navy rounded square with a blueprint corner bracket and an
orange-to-magenta pointer nib. It is drawn in code from the design spec (coordinates and colours are
constants in `icon-gen/src/art.rs`); no bitmap is read. It uses no existing product's logo or style.

To regenerate it, run:

```
cargo run --release --manifest-path packaging/icon-gen/Cargo.toml
cargo run --release --manifest-path packaging/icon-gen/Cargo.toml -- android android/app/src/main/res
```

The first writes `packaging/icons/png/eskiz-{16,32,48,64,128,256,512,1024}.png`,
`packaging/icons/eskiz.icns` (PNG-based ICNS elements, 16 to 1024 px) and
`packaging/icons/eskiz.ico` (16, 32, 48 and 256 px); pass a directory as the first argument to write
somewhere else. The second writes the Android adaptive icon layers (background, foreground inside the
66 dp safe circle, monochrome) for the five densities. `... -- compare <reference.png>` renders the
icon at the reference's size and prints the difference. `icon-gen` is its own workspace root, so it
stays out of the app build. Copy `packaging/icons/png/eskiz-512.png` to `android/store/icon-512.png`.

The icon is the input for the bundles below. Change the constants in `icon-gen/src/art.rs` to change
the artwork, then regenerate and rebuild the packages.

## macOS: universal `.app` and DMG

```
./packaging/macos/build.sh
```

Requirements: Xcode command line tools (`lipo`, `codesign`, `hdiutil`). If the Homebrew
rustup is at `/opt/homebrew/opt/rustup/bin` (and not running in CI), it is put first on
`PATH`. The script adds the `aarch64-apple-darwin` and `x86_64-apple-darwin` targets if
they are missing.

What it does:

1. Builds `eskiz-app` for both architectures and joins them with `lipo`.
2. Assembles `Eskiz.app` with `Info.plist.in`. The version comes from `Cargo.toml`, the bundle
   id is `app.eskiz.Eskiz`, and the icon is `eskiz.icns`.
3. Ad-hoc signs the bundle (`codesign --sign -`) and checks the signature.
4. Makes `Eskiz-<v>-macos-universal.dmg` with `hdiutil`, containing `Eskiz.app` and an
   `Applications` symlink.

Document types. `.procreate` (Eskiz's native format) is opened by Eskiz with the Alternate handler rank (it does not take over the type), and
its UTI `app.eskiz.document` is exported. `.brush`, `.swatches` and `.psd` are import types:
they are declared as imported UTIs (`app.eskiz.brush`, `app.eskiz.swatches`,
`com.adobe.photoshop-image`) and set to the Alternate handler rank, so Eskiz does not take
them over by default.

The bundle is ad-hoc signed, not Developer ID signed or notarised. A downloaded copy may
be blocked by Gatekeeper on first launch. The user can allow it in System Settings, or run
`xattr -dr com.apple.quarantine /Applications/Eskiz.app`. Notarisation needs an Apple
developer account and is not set up.

## Windows: zip

From a Windows shell with the MSVC Rust toolchain:

```
powershell -ExecutionPolicy Bypass -File packaging\windows\build.ps1
```

Options: `-SkipBuild` packages an existing `target\x86_64-pc-windows-msvc\release\eskiz.exe`.

The script builds `x86_64-pc-windows-msvc`, runs `eskiz --selftest`, and writes
`Eskiz-<v>-windows-x64.zip`. The zip holds one folder with `eskiz.exe` and `eskiz.ico`.

`eskiz-app/build.rs` embeds the icon and version info into `eskiz.exe`. It uses
`winresource`. The check is on the target OS, so non-Windows builds skip it. The resource
compiler is `rc.exe` (Windows SDK) on MSVC, and `windres` on `windows-gnu`.

Smoke test of the real window (no Windows PC needed: the public repo's `Windows smoke` workflow
runs it on a GitHub-hosted `windows-latest` runner and uploads the screenshots):

```
pwsh -File packaging\windows\smoke.ps1 [-Exe target\release\eskiz.exe] [-OutDir target\windows-smoke] [-StrictInput]
```

It runs `--version`, `--selftest` (which also writes a `selftest.procreate` and names its path), `--diagnose`
and `--diagnose --window`, then opens the window on `selftest.procreate`, takes screenshots
(desktop and `PrintWindow`), paints one stroke with real mouse input, presses Ctrl+Z and closes the
window. It fails when a command fails, the window does not appear, or the app exits by itself. A
stroke or undo that changes no pixels is a warning unless `-StrictInput` is given. The report is
`smoke-report.md` in the output folder.

Cross-build from macOS (mingw-w64 is installed):

```
cargo build --release -p eskiz-app --target x86_64-pc-windows-gnu
```

The linker is set in `.cargo/config.toml`. `winresource` calls `x86_64-w64-mingw32-windres`
for the resource step. The resulting `target/x86_64-pc-windows-gnu/release/eskiz.exe` has
an `.rsrc` section.

Windows is not code-signed. SmartScreen may warn on first run.

## Linux: AppImage and tarball

Build the binary against glibc 2.28, so it runs on older distributions:

```
cargo zigbuild --release -p eskiz-app --target x86_64-unknown-linux-gnu.2.28
./packaging/linux/build-appimage.sh
```

`build-appimage.sh` does three things:

1. Assembles `dist/release/Eskiz.AppDir` with `AppRun`, `eskiz.desktop`, the icon, the
   hicolor sizes (16 to 512) and the shared MIME definitions. No system libraries are bundled
   (the build machine's libxkbcommon needs a newer glibc than the 2.28 baseline). It then checks
   every ELF file in the AppDir with `objdump -T` and stops if one needs a GLIBC symbol newer
   than 2.28.
2. Runs `appimagetool` to make `Eskiz-<v>-x86_64.AppImage`. The tool is found through
   `$APPIMAGETOOL` or `PATH`. If it is missing locally the step is skipped. When `CI` is set,
   a missing tool is an error.
3. Writes `Eskiz-<v>-linux-x86_64.tar.gz`: `bin/eskiz`, `share/` (desktop entry, icons,
   MIME), `install.sh` and `DEPENDENCIES.txt`. Users run `./install.sh` to install under
   `~/.local`, or set `PREFIX` for a system-wide install. The tarball bundles no libraries.

### Runtime dependencies

Eskiz loads some system libraries when the window opens (winit uses dlopen). Without them
the window cannot start. At startup, Eskiz checks the libraries for the session it will use
and, if any are missing, prints the packages to install and exits with status 2. The
tarball's `DEPENDENCIES.txt` has the full list. The short form:

| Need | Debian / Ubuntu | Fedora |
|---|---|---|
| X11 session (`DISPLAY`) | `libxkbcommon-x11-0 libxkbcommon0 libx11-xcb1` | `libxkbcommon-x11 libxkbcommon libX11-xcb` |
| Wayland session (`WAYLAND_DISPLAY`) | `libxkbcommon0 libwayland-client0` | `libxkbcommon libwayland-client` |
| OpenGL and Vulkan drawing | `libgl1 libvulkan1 libgl1-mesa-dri mesa-vulkan-drivers` | `mesa-libGL vulkan-loader mesa-dri-drivers mesa-vulkan-drivers` |

The AppImage bundles no libraries, so it needs the whole list.

To get `appimagetool` locally, download
`https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage`,
make it executable, and set `APPIMAGETOOL` to its path. Set `APPIMAGE_EXTRACT_AND_RUN=1` on
machines without FUSE.

Shared MIME: `eskiz.xml` defines `application/x-eskiz-document` (`*.procreate`),
`application/x-eskiz-brush` (`*.brush`) and `application/x-eskiz-swatches` (`*.swatches`),
all subclasses of `application/zip`. `.psd` uses the existing
`image/vnd.adobe.photoshop` type. `eskiz.desktop` lists all four in `MimeType=`.

## CI release

`.github/workflows/release.yml` runs on `v*` tags only:

| Job | Runner | Output |
|---|---|---|
| check-version | ubuntu-latest | fails if the tag differs from `Cargo.toml` |
| macos | macos-latest | universal DMG (`build.sh`) |
| windows | windows-latest | zip (`build.ps1`) |
| linux | ubuntu-latest | AppImage and tarball (zigbuild glibc 2.28, `build-appimage.sh`) |
| release | ubuntu-latest | attaches all files and `SHA256SUMS.txt` with `softprops/action-gh-release` |

The Linux job installs the same apt packages as `ci.yml`, plus `libxkbcommon-x11-0` for the
bundled libraries. `ci.yml` also runs a launch smoke test under Xvfb: Eskiz must still be
running after 20 seconds (`timeout` exits 124). It downloads `appimagetool`
from the `continuous` release, which is not pinned, so a new upstream build could change the
result. Pin a version there if reproducible AppImages matter.

## Not verified locally

- The AppImage (no `appimagetool` on this machine), and whether it launches on a real
  distribution.
- `windows/build.ps1` has not been run, because PowerShell is not installed on this Mac. The
  MSVC build, the self-test and the zip are unverified until the Windows CI job runs.
  The mingw cross-build was checked for the icon and version resources.
- Linux desktop integration (MIME and `.desktop`) on a live desktop.
- The missing-library message on a real Debian or Fedora system without `libxkbcommon-x11`.
  The Fedora package names are from memory and have not been checked on a Fedora system.
- Gatekeeper behavior of the ad-hoc signed app on other Macs.

## Licences and notices in every package

`THIRD_PARTY_NOTICES.md` (made by `tools/gen_notices.sh` with cargo-about), `LICENSE-MIT` and `LICENSE-APACHE` go
into each artifact: macOS `Eskiz.app/Contents/Resources` (before signing), the Windows zip (next to `eskiz.exe`),
the AppImage (`usr/share/doc/eskiz/`), the Linux
tarball (`share/doc/eskiz/`) and the Android APK (`assets/licenses/`, from `THIRD_PARTY_NOTICES-android.md`,
copied by the Gradle `copyLicenses` task). `tools/check_notices.sh` inspects whichever packages exist under
`dist/release` (and APKs) and fails when a file is missing. Regenerate the notices whenever `Cargo.lock` changes.
