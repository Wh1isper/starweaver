# Install and Update Starweaver Desktop

> [!WARNING]
> Starweaver Desktop is WIP. CI, active maintenance, release packaging, and updater publication are
> paused. No current Desktop package is supported or published; the remaining page is retained as
> historical verification guidance for previously produced prerelease artifacts.

Starweaver Desktop release packages are currently **not signed with platform publisher
certificates**. macOS Gatekeeper and Windows SmartScreen can therefore warn even when an official
package is unchanged. Verify the download before choosing any platform bypass.

Download only from the official release page:

<https://github.com/Wh1isper/starweaver/releases>

Do not use a package forwarded by another person or hosted on a mirror. Confirm that the release URL
uses the exact `Wh1isper/starweaver` owner and repository over HTTPS, and select the package for your
platform:

| Platform            | Package               |
| ------------------- | --------------------- |
| macOS Intel         | `x86_64` `.dmg`       |
| macOS Apple Silicon | `aarch64` `.dmg`      |
| Windows x64         | `x86_64` NSIS `.exe`  |
| Linux x86_64        | `.AppImage` or `.deb` |

Linux ARM64 and Windows ARM64 packages are not currently published.

## What Each Verification Means

These mechanisms are separate and are not substitutes for one another:

| Mechanism                  | What it establishes                                                                                             | What it does not establish                                                                                                                                                                                                                     |
| -------------------------- | --------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| OS code signing            | Apple Developer ID/notarization or Windows Authenticode publisher identity recognized by the OS                 | **Not configured for current Starweaver Desktop packages.** This is why platform warnings appear.                                                                                                                                              |
| Tauri updater signature    | An update was authorized by the Starweaver project key embedded in Desktop                                      | It does not sign the app for Apple or Microsoft and does not remove OS warnings. Creating this project key is free, but Tauri's cryptographic signature is still required for updater-delivered artifacts and is never replaced by a checksum. |
| SHA-256 checksum           | The downloaded bytes match the filename recorded in `desktop-checksums.txt`                                     | A checksum alone does not identify the publisher when the package and checksum came from the same location.                                                                                                                                    |
| GitHub artifact provenance | GitHub verifies that the exact artifact digest was produced for `Wh1isper/starweaver` by its release automation | It is not Apple notarization, Windows Authenticode, antivirus scanning, or a guarantee that source code is harmless.                                                                                                                           |

Tauri verifies updater signatures automatically; users do not bypass that check. The initial manual
install, and any manual fallback update, should be verified with both SHA-256 and GitHub provenance
before bypassing an OS warning.

## Verify Before Installing

Download the package and `desktop-checksums.txt` from the same release. Keep their original filenames.
Install a current [GitHub CLI](https://cli.github.com/) to verify the artifact attestation.

### macOS

From the directory containing the downloads, replace the example filename with the exact asset name:

```bash
PACKAGE='Starweaver_VERSION_ARCH.dmg'
shasum -a 256 "$PACKAGE"
grep -F "  $PACKAGE" desktop-checksums.txt
gh attestation verify "$PACKAGE" --repo Wh1isper/starweaver
```

The SHA-256 printed by `shasum` must exactly match the value on the `desktop-checksums.txt` line, and the
attestation command must succeed for `Wh1isper/starweaver`.

### Windows PowerShell

Replace the example filename with the exact asset name:

```powershell
$Package = "Starweaver_VERSION_x64-setup.exe"
Get-FileHash $Package -Algorithm SHA256
Select-String -Path desktop-checksums.txt -SimpleMatch ("  " + $Package)
gh attestation verify $Package --repo Wh1isper/starweaver
```

The two SHA-256 values must match without regard to letter case, and the attestation command must
succeed for `Wh1isper/starweaver`.

### Linux

From the directory containing the downloads, replace the example filename with the exact `.AppImage`
or `.deb` asset name:

```bash
PACKAGE='Starweaver_VERSION_amd64.AppImage'
sha256sum "$PACKAGE"
grep -F "  $PACKAGE" desktop-checksums.txt
gh attestation verify "$PACKAGE" --repo Wh1isper/starweaver
```

The SHA-256 values must exactly match, and the attestation command must succeed for
`Wh1isper/starweaver`.

If a checksum differs, a filename has no entry, or provenance verification fails, **do not run the
package and do not bypass the warning**. Delete it and download it again from the official release.
If verification still fails, use a different published release or build from reviewed source and
report the release asset problem.

## Install and Handle Platform Warnings

Perform these steps only after verification succeeds.

### macOS

1. Open the `.dmg` and drag **Starweaver** to **Applications**.
2. In Finder, Control-click **Starweaver** in Applications, select **Open**, then select **Open** in
   the confirmation. This creates an exception only for this app.
3. If macOS instead offers **Open Anyway**, use **System Settings > Privacy & Security > Open
   Anyway**, then confirm.

Do not disable Gatekeeper globally. If a verified app is still reported as damaged and neither
per-app option is available, remove quarantine only from this installed app, then open it again:

```bash
xattr -dr com.apple.quarantine /Applications/Starweaver.app
```

Recheck the command path before running it; do not apply recursive `xattr` changes to `/Applications`
or another broad directory.

### Windows

1. Run the verified NSIS `.exe`.
2. If SmartScreen shows **Windows protected your PC**, select **More info**, confirm that the file is
   the one you verified, then select **Run anyway**.
3. If User Account Control shows **Unknown publisher**, confirm the same filename and choose **Yes**
   only after the checks above succeeded.

Do not disable SmartScreen or antivirus globally. If **Run anyway** is unavailable because the device
is managed, do not weaken the policy; ask the administrator to approve the verified artifact or build
from reviewed source.

### Linux

For AppImage:

```bash
chmod +x ./Starweaver_VERSION_amd64.AppImage
./Starweaver_VERSION_amd64.AppImage
```

For Debian or Ubuntu:

```bash
sudo apt install ./starweaver_VERSION_amd64.deb
```

Some graphical installers label a local package as untrusted because it did not come from a signed
system repository. Install it only after the checks above. Do not use `--no-sandbox` or disable system
package verification. If an AppImage cannot start because its runtime support is unavailable, use the
verified `.deb` package instead.

## Update, Fallback, and Recovery

The **Updates** section in Desktop Settings manages two separate channels.

### RPC runtime update

Every Desktop package retains its exact bundled RPC runtime as a recovery fallback. An independently
published RPC is offered only after Desktop verifies the Starweaver project signature and confirms
that its target, host protocol, launch schema, Desktop version range, storage generation, byte size,
and SHA-256 digest are compatible.

Installing a runtime candidate downloads it into a private versioned directory and runs an isolated
initialize probe. It does not stop the current host or interrupt an active run. The verified runtime
is selected the next time the entire Desktop application starts. Quit and reopen Desktop when you are
ready to activate it.

**Roll back runtime** selects the previous verified runtime for the next start. If none exists, it
selects the bundled runtime. Current runtime updates are not allowed to change the database schema, so
this rollback changes only the executable selection. If a managed runtime cannot be fully reverified
or fails to start and initialize, Desktop automatically makes one fresh startup attempt with the
bundled sidecar.

### Desktop application update

A Desktop update is checked against the fixed official GitHub release feed. Tauri downloads and
verifies the mandatory Starweaver project signature before installation. Desktop then shows a native
confirmation explaining that the package is not signed by Apple or Microsoft. If you approve it,
Desktop coordinates RPC shutdown, installs the retained update, and restarts.

The project signature proves that the update was authorized for this update channel; it does not
remove Gatekeeper, SmartScreen, or unknown-publisher warnings. If the operating system warns again
after the update, apply only the per-application steps documented above. Do not disable a platform
security feature globally.

### Manual fallback

If an in-app check or install is unavailable:

1. Read the release notes on the official GitHub Release.
2. Quit Starweaver Desktop completely.
3. Download the new package, `desktop-checksums.txt`, and verify both SHA-256 and GitHub provenance as
   described above.
4. Install over the existing application: replace the app from the `.dmg`, rerun the NSIS installer,
   replace the AppImage, or run `apt install` on the new `.deb`.
5. Keep the previous verified installer until the new version opens successfully.

If the new version will not start, reinstall a previously verified official package using the same
platform steps. Do not delete Starweaver user data as an initial recovery action. A downgrade may be
unable to read data written by a newer schema; check both releases' notes and restore a compatible
backup rather than forcing an older application to open newer data.

An updater or runtime-manifest signature failure is not a platform-warning case: it cannot and must
not be bypassed. Use a manually downloaded package only after verifying its checksum and GitHub
provenance, remain on the installed version, and report the failed update.
