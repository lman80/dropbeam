## Download DropBeam

| Platform | File |
|---|---|
| **macOS** (Apple Silicon & Intel) | `DropBeam_<version>_universal.dmg` — open it, drag DropBeam to Applications |
| **Windows** (x64) | `DropBeam_<version>_x64-setup.exe` (or the `.msi` for managed installs) |
| **Windows on ARM** | `DropBeam_<version>_arm64-setup.exe` (when present) |
| **Linux** (x86_64 / aarch64) | `.deb` (Debian/Ubuntu), `.rpm` (Fedora/openSUSE) or `.AppImage` (any distro) |

**Linux AppImage:** needs FUSE 2 — on Ubuntu 22.04+ run `sudo apt install libfuse2` (24.04: `libfuse2t64`), then `chmod +x DropBeam_*.AppImage` and run it.

**If the app says it can't be verified** (only on builds that aren't code-signed yet):
- **macOS:** open it once → **Done**, then **System Settings → Privacy & Security → Open Anyway**.
- **Windows:** SmartScreen → **More info → Run anyway**.

After the first install, **updates install themselves** from inside the app.

Privacy: see [PRIVACY.md](https://github.com/lman80/dropbeam/blob/main/PRIVACY.md) — diagnostics are on by default and can be turned off in Settings.
