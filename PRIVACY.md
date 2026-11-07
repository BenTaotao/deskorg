# Privacy policy

This program will not transfer any information to other networked systems unless specifically requested by the user or the person installing or operating it.

DeskOrg has no accounts, telemetry, analytics, cloud sync, or update service.

## Local data

- `%APPDATA%\DeskOrg\config.json` — settings, fence layout, and the selected wallpaper path.
- `%APPDATA%\DeskOrg\undo.json` — the undo log for the last file organization.
- `HKCU\Software\Microsoft\Windows\CurrentVersion\Run\DeskOrg` — only when the optional auto-start setting is enabled.

## File operations

DeskOrg scans the current user's desktop and, after explicit confirmation, moves files into `Desktop\DeskOrg\<category>\`. It does not delete files and does not upload file names or contents.

## Uninstallation

Exit DeskOrg, delete `DeskOrg.exe` and the `%APPDATA%\DeskOrg` directory, and remove the `DeskOrg` value under `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` if auto-start was enabled.
