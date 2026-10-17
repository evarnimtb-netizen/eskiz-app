# Eskiz

Eskiz is a free, open source painting app for macOS, Windows, Linux and (experimentally) Android. It is written in Rust. It has layers with blend modes and masks, a pressure and tilt aware brush engine, selections and transforms, text layers, drawing guides, animation, 3D model painting, and a GPU-accelerated canvas. All icons, brushes and textures are drawn in code.

**Status: first public preview (0.1.0).** It is usable, but expect rough edges. Please file issues.

> Eskiz is an independent project. It is not affiliated with or endorsed by the makers of any other app; `.procreate` files are supported as a file format for compatibility.

## Install

Download a package from the [Releases page](https://github.com/evarnimtb-netizen/eskiz-app/releases) and check it against `SHA256SUMS`. These preview builds are **not code-signed** (signing certificates cost money and this is a volunteer project), so your system will warn you the first time. Here is how to open each one.

**macOS (universal, Apple silicon and Intel).** Open the `.dmg` and drag Eskiz to Applications. The app is ad-hoc signed but not notarized. On first launch, Control-click Eskiz, choose Open, and confirm. If macOS still refuses, open System Settings, Privacy & Security, and choose Open Anyway, or run `xattr -dr com.apple.quarantine /Applications/Eskiz.app` in Terminal.

**Windows (x64).** Unzip the `.zip` and run `eskiz.exe`. If SmartScreen says "Windows protected your PC", choose More info, then Run anyway. Windows builds have not yet been tested on real hardware beyond automated checks.

**Linux (x86_64).** Either download the `.AppImage`, make it executable (`chmod +x Eskiz-*.AppImage`) and run it, or unpack the `.tar.gz` and run `./install.sh`. The AppImage needs a few system libraries for graphics (Vulkan or OpenGL drivers and X11 or Wayland); if one is missing Eskiz tells you which packages to install. The tarball lists its requirements in `DEPENDENCIES.txt`.

**Android (arm64, experimental).** Download the `.apk` on your device and allow installing from this source when asked, or install it with `adb install Eskiz-*.apk`. It needs Android 10 or newer. It is signed with a public debug key, so treat it as a preview: there is no system file picker, share sheet or clipboard integration yet.

## Build from source

You need the Rust toolchain installer, [rustup](https://rustup.rs). The right Rust version is picked from `rust-toolchain.toml`.

```
git clone https://github.com/evarnimtb-netizen/eskiz-app.git
cd eskiz
cargo run --release -p eskiz-app
```

On Linux, install the development libraries first (Debian or Ubuntu names):

```
sudo apt-get install libxkbcommon-dev libxkbcommon-x11-0 libgl1-mesa-dev libwayland-dev \
  libx11-dev libxcursor-dev libxrandr-dev libxi-dev fonts-dejavu-core mesa-vulkan-drivers
```

Run the checks that CI runs:

```
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo build --release -p eskiz-app --locked
./target/release/eskiz --selftest
```

`eskiz --diagnose` prints a report about your graphics setup that is useful in bug reports. Packaging scripts for each platform are in `packaging/`, and the Android project is in `android/`. `INTEGRATION.md` explains how the crates fit together.

## What is and is not in 0.1.0

MP4 export is not included in the distributed builds (GIF, APNG and PNG sequences are). Details are in `CHANGELOG.md`.

Eskiz reads and writes `.procreate` files for compatibility. The name refers only to that file format.

## Contributing

Bug reports, feature ideas and patches are welcome. Read `CONTRIBUTING.md` first; contributions need a DCO sign-off. Everyone taking part follows the [Code of Conduct](CODE_OF_CONDUCT.md). To report a security problem, see `SECURITY.md`. Eskiz does not collect any data; see `PRIVACY.md`.

## Support

Eskiz is free and always will be. If you would like to support its development, a donation link will appear here. Donating is optional and never unlocks features or priority.

<!-- Donation link placeholder: add it here (and in .github/FUNDING.yml) when the maintainer has an account. -->

## License

Licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option. Third-party notices are in `THIRD_PARTY_NOTICES.md` and are included in every package.

MP4 export uses the OpenH264 codec. Cisco's binary builds of OpenH264 come with a patent license that applies only to the unmodified binary downloaded from Cisco. A copy compiled from source does not carry that license, so distributed builds ship without MP4 export. Developers can enable it with `--features mp4` and are responsible for any codec licensing that applies to them.
