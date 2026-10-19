//! MLX Ink – native side. Deliberately small:
//!  * `export_text` / `export_binary` – the backend shows a Save dialog and writes only the file picked
//!                     by the user, so the webview never gets a "write anywhere" capability.
//!  * `folders::*` – project-folder access is limited to paths chosen by the user through native dialogs.
//!  * `store_*`      – a tiny key/value mirror of the app's localStorage data in the app-data folder.
//!                     Keys are restricted to `klapka.<[A-Za-z0-9._-]>` and map to `<key>.json` files.
//!  * `vault_*`      – the app's primary disk store for projects (see vault.rs), separate folder `mlx-vault`.

mod folders; // native user-selected project folders
mod fonts; // installed font names for the script font picker (src/fonts.rs)
mod vault; // disk store for the app's projects (src/vault.rs): vault_status/list/get/put/del, folder mlx-vault

use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{ErrorKind, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use base64::{engine::general_purpose::STANDARD, Engine};
use tauri::{AppHandle, Manager};
use tauri_plugin_dialog::DialogExt;

const KEY_PREFIX: &str = "klapka.";
const MAX_KEY_LEN: usize = 128;

fn valid_key(key: &str) -> bool {
    key.starts_with(KEY_PREFIX)
        && key.len() <= MAX_KEY_LEN
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-')
}

fn data_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("data");
    fs::create_dir_all(&dir).map_err(|e| format!("cannot create data folder: {e}"))?;
    Ok(dir)
}

fn key_path(dir: &Path, key: &str) -> Result<PathBuf, String> {
    if !valid_key(key) {
        return Err("invalid storage key".into());
    }
    Ok(dir.join(format!("{key}.json")))
}

fn write_export(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let leaf = path
        .file_name()
        .ok_or_else(|| "invalid export file path".to_string())?
        .to_string_lossy();
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let tmp = path.with_file_name(format!(".{leaf}.{}.{}.tmp", std::process::id(), stamp));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
            .map_err(|e| e.to_string())?;
        file.write_all(bytes).map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        fs::rename(&tmp, path).map_err(|e| e.to_string())
    })();
    if result.is_err() {
        let _ = fs::remove_file(tmp);
    }
    result
}

/// Shows the native Save dialog and writes `contents` (UTF-8) to the chosen file.
/// Returns the chosen path, or `None` if the user cancelled.
#[tauri::command]
async fn export_text(
    app: AppHandle,
    name: String,
    contents: String,
) -> Result<Option<String>, String> {
    let mut dialog = app.dialog().file().set_file_name(name.clone());
    if let Some(ext) = Path::new(&name).extension().and_then(|e| e.to_str()) {
        dialog = dialog.add_filter(ext.to_uppercase(), &[ext]);
    }
    let Some(picked) = dialog.blocking_save_file() else {
        return Ok(None);
    };
    let path = picked.into_path().map_err(|e| e.to_string())?;
    write_export(&path, contents.as_bytes()).map_err(|e| format!("cannot write file: {e}"))?;
    Ok(Some(path.to_string_lossy().into_owned()))
}

/// Shows the native Save dialog and writes base64-encoded bytes to the chosen file.
#[tauri::command]
async fn export_binary(
    app: AppHandle,
    name: String,
    contents: String,
) -> Result<Option<String>, String> {
    let bytes = STANDARD
        .decode(
            contents
                .split_once(";base64,")
                .map_or(contents.as_str(), |(_, data)| data),
        )
        .map_err(|e| format!("invalid binary export: {e}"))?;
    let mut dialog = app.dialog().file().set_file_name(name.clone());
    if let Some(ext) = Path::new(&name).extension().and_then(|e| e.to_str()) {
        dialog = dialog.add_filter(ext.to_uppercase(), &[ext]);
    }
    let Some(picked) = dialog.blocking_save_file() else {
        return Ok(None);
    };
    let path = picked.into_path().map_err(|e| e.to_string())?;
    write_export(&path, &bytes).map_err(|e| format!("cannot write file: {e}"))?;
    Ok(Some(path.to_string_lossy().into_owned()))
}

/// Returns every stored `klapka.*` value (raw strings, exactly as localStorage held them).
#[tauri::command]
async fn store_load_all(app: AppHandle) -> Result<HashMap<String, String>, String> {
    let dir = data_dir(&app)?;
    let mut out = HashMap::new();
    for entry in fs::read_dir(&dir).map_err(|e| e.to_string())?.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue; // also skips leftover *.tmp files
        }
        let Some(key) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if !valid_key(key) {
            continue;
        }
        if let Ok(value) = fs::read_to_string(&path) {
            out.insert(key.to_string(), value);
        }
    }
    Ok(out)
}

/// Atomic write: temp file -> flush to disk -> rename over the old file, so a crash or power
/// loss can never leave a half-written script behind.
#[tauri::command]
async fn store_set(app: AppHandle, key: String, value: String) -> Result<(), String> {
    let dir = data_dir(&app)?;
    let path = key_path(&dir, &key)?;
    let tmp = dir.join(format!("{key}.json.tmp"));
    let write = || -> std::io::Result<()> {
        let mut f = File::create(&tmp)?;
        f.write_all(value.as_bytes())?;
        f.sync_all()?;
        fs::rename(&tmp, &path)
    };
    write().map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("cannot save {key}: {e}")
    })
}

#[tauri::command]
async fn store_remove(app: AppHandle, key: String) -> Result<(), String> {
    let dir = data_dir(&app)?;
    let path = key_path(&dir, &key)?;
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("cannot delete {key}: {e}")),
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            export_text,
            export_binary,
            store_load_all,
            store_set,
            store_remove,
            folders::choose_project_folder,
            folders::list_project_folders,
            folders::write_project_folder,
            folders::open_project_folder,
            fonts::list_system_fonts,
            vault::vault_status,
            vault::vault_list,
            vault::vault_get,
            vault::vault_put,
            vault::vault_del
        ])
        .run(tauri::generate_context!())
        .expect("error while running MLX Ink");
}
