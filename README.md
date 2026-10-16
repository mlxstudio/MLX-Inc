# MLX Ink – Tauri 2 desktop build

`index.html` is your existing application (5 small edits, see "What changed"). Everything else is the
Tauri/Vite shell around it. Your original file is untouched.

## One-time setup (Windows)

```powershell
cd mlx-ink
npm install
npx tauri icon path\to\your-logo.png     # creates src-tauri\icons\* (required, see src-tauri\icons\README.txt)
```

Align versions with your working starter if you like (`@tauri-apps/cli`, `@tauri-apps/api`, `vite`,
`typescript` in `package.json`). **Tauri >= 2.1 is required** (`useHttpsScheme` in tauri.conf.json).

## Run

```powershell
npm run tauri dev        # desktop app with hot reload
npm run dev              # browser only (http://localhost:1420) - same app, original download behaviour
```

## Build the Windows installer

```powershell
npm run tauri build
```

Result: `src-tauri\target\release\bundle\nsis\MLX Ink_0.1.2_x64-setup.exe`
(per-user install, no admin rights; WebView2 is downloaded by the installer only if missing).
Standalone exe without installer: `src-tauri\target\release\mlx-ink.exe`.
The executable, installer, and uninstaller use the dark app icon with its teal feather. The NSIS
wizard uses a light neutral background for readable native controls, Segoe UI text, and matching
branded header and welcome/finish artwork. Its Add/Remove Programs icon points to the uninstaller
executable. The theme is in
`src-tauri\nsis\installer.nsi`. To replace the app mark, provide a square PNG and run
`npx tauri icon path\to\your-logo.png --output src-tauri\icons`; update the two installer bitmaps
in `src-tauri\icons` to match the new artwork.

## Set up GitHub releases and app updates

The desktop app checks for updates at startup and has a **Check for Updates…** option in the
home-screen menu. When a signed release is available, the app downloads and installs it
automatically; a toast indicates when the download starts. GitHub Actions builds and publishes a
signed Windows release whenever a version tag such as `v0.1.2` is pushed.

Before the first release:

1. In GitHub, open **Settings → Secrets and variables → Actions**. Add an Actions secret named
   `TAURI_SIGNING_PRIVATE_KEY` containing the private updater key that matches the public key in
   `src-tauri\tauri.conf.json`. If you do not have that private key, generate a new pair locally
   with `npx tauri signer generate -w $HOME\.tauri\mlx-ink.key`, then put the generated public key
   in `plugins.updater.pubkey` in the Tauri config. Keep the private key somewhere safe; never
   commit or share it. If you set a key password, also add
   `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` with that password.
2. Confirm the updater endpoint in `src-tauri\tauri.conf.json` points to this repository's
   `releases/latest/download/latest.json`. The repository and its release assets must be publicly
   downloadable for the unauthenticated updater to reach them.
3. For each release, update the version in `src-tauri\tauri.conf.json`, `src-tauri\Cargo.toml`,
   and `package.json` to the same new version. Commit the changes, create a matching `v<version>`
   tag (for example, `v0.1.2`), then push the commit and tag. The workflow checks the tag against
   the Tauri config version before building.
4. Wait for the **Release** workflow to finish. It creates a GitHub Release and uploads the
   Windows installer and signed updater manifest/assets used by existing app installations.

Keep the private key safe and backed up. Rotating it requires updating the configured public key
and distributing a newly signed app build through a separate channel, because existing installs
trust the current public key.

## What changed in index.html

1. Google Fonts `<link>`s -> `<link rel="stylesheet" href="/src/fonts.css">` (Courier Prime bundled locally via
   `@fontsource/courier-prime`, same 400 / 700 / 400-italic faces; works offline).
2. `downloadText()` -> native Save dialog on desktop, original download link in a browser.
3. Export sheet "Download" button -> uses `downloadText()` (CSV byte-order mark preserved as `\uFEFF`).
4. Project export and version export toast "Saved ..." only after a real save (not on cancel).
5. `<script type="module" src="/src/desktop.ts">` appended.

## Where data lives

* Primary store is unchanged: WebView2 `localStorage` (`klapka.*` keys), `%LOCALAPPDATA%\com.mlxink.desktop\EBWebView`.
* Safety mirror (new): every `klapka.*` write is copied atomically to `%APPDATA%\com.mlxink.desktop\data\<key>.json`.
  If the WebView profile is ever reset, scripts are restored from there at next start. Nothing is overwritten
  from disk if localStorage already has a value.
* **Moving scripts from the web version:** localStorage is per-origin, so the browser's scripts are not visible to
  the desktop app. In the web version use the existing *Project file (.json)* export per script, then import it in
  the desktop app (home screen -> import). No data format was changed.
* Choose the real `identifier` (tauri.conf.json) **before** first release. `com.mlxink.desktop` is a placeholder;
  changing it later moves both data folders.

## Manual test checklist (not yet run on a real Windows build)

- [ ] App starts, Courier Prime renders (including Czech characters), layout identical to browser.
- [ ] Create/edit a script, close the window right away, reopen: last keystrokes present.
- [ ] Export: Fountain, FDX, CSV, .project.json, version `.fountain` -> native Save dialog, file opens correctly; cancel shows no toast.
- [ ] Import a .fountain / .fdx / .json via "Open file...".
- [ ] Drag scenes to reorder in the navigator and in the board.
- [ ] Print / Save as PDF (page layout, watermark, title page, page numbers).
- [ ] Copy buttons (clipboard), image import for watermark, dark/light theme.
- [ ] Delete `EBWebView` folder, restart: scripts come back from `%APPDATA%\...\data`.
