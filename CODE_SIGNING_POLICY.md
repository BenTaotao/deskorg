# Code signing policy

Free code signing provided by [SignPath.io](https://signpath.io), certificate by [SignPath Foundation](https://signpath.org).

## Project

- Project: DeskOrg
- Repository: <https://github.com/BenTaotao/deskorg>
- License: MIT
- Releases: <https://github.com/BenTaotao/deskorg/releases>

## Team roles

DeskOrg is currently maintained by a single maintainer. The same person holds all signing roles until additional maintainers are added.

- Authors / Committers: [@BenTaotao](https://github.com/BenTaotao) — may modify source code.
- Reviewers: [@BenTaotao](https://github.com/BenTaotao) — reviews pull requests from non-committers.
- Approvers: [@BenTaotao](https://github.com/BenTaotao) — approves every signing request.

All team members must use multi-factor authentication for GitHub and SignPath.

## Release and signing process

- Windows binaries are built from the source code in this repository by the GitHub Actions workflow in `.github/workflows/build.yml`.
- Only artifacts produced by that automated build are submitted for signing.
- Every signing request is manually approved by an Approver.
- A signed binary is published only after the Authenticode signature and SHA256 checksum are verified.
- Current release binaries are unsigned while the project applies for SignPath Foundation code signing.

## Privacy

This program will not transfer any information to other networked systems unless specifically requested by the user or the person installing or operating it.

DeskOrg has no accounts, telemetry, analytics, cloud sync, or update service. Configuration and undo history are stored locally in `%APPDATA%\DeskOrg`. The optional auto-start setting writes the current user's `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` value and can be disabled in the app.

See [PRIVACY.md](PRIVACY.md) for the full privacy policy.

## System changes and uninstallation

DeskOrg is a portable application. It does not install a service or driver.

- File organization moves desktop items into `Desktop\DeskOrg\<category>\` only after the user confirms the operation. Files are not deleted.
- Fence mode hides the native Windows desktop icons through the standard shell command `WM_COMMAND 0x7402`; leaving fence mode restores the icons that DeskOrg hid.
- Optional auto-start can be disabled in Settings, or by deleting the `DeskOrg` value under `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`.
- To uninstall, exit DeskOrg and delete `DeskOrg.exe` and the `%APPDATA%\DeskOrg` directory. No other system components are installed.
- DeskOrg does not inject into Explorer, install a global input hook, or require administrator privileges by default.

## Reporting a policy violation

If a binary signed with a SignPath Foundation certificate appears to violate this policy, report it to support@signpath.io with a concise description and evidence.
