//! Disk store for MLX Ink (schema 3). The front end (Vault in index.html) keeps one JSON value per key
//! ("klapka.proj.<id>", "klapka.idx", "klapka.ui", "klapka.mig", "klapka.bak.*") and calls these commands.
//! Files live in <app data dir>/mlx-vault/<key>.json. That folder is NOT part of the webview profile, so
//! resetting the profile cannot touch it. It is separate from the files written by the existing
//! store_set / store_remove / store_load_all commands (the localStorage mirror in desktop.ts): both keep
//! working side by side, and the command names here (vault_*) do not clash with those.
//!
//! Safety rules:
//!  * atomic write: the value goes to <file>.tmp, is fsynced, then renamed over the target (a crash leaves
//!    the old file or the new file, never half of one);
//!  * <file>.bak is a copy of the previous good file, refreshed at most every 10 minutes;
//!  * a read that finds the main file empty or cut off falls back to <file>.bak;
//!  * only keys that start with "klapka." and contain [A-Za-z0-9._-] are accepted (no paths).
//!
//! Wiring (needs a rebuild): in src-tauri/src/lib.rs (or main.rs) add `mod vault;` and put
//! vault::vault_status, vault::vault_list, vault::vault_get, vault::vault_put, vault::vault_del
//! into tauri::generate_handler![...] next to the existing commands. No new crates, no capability changes
//! (app commands are allowed by default unless build.rs uses AppManifest::commands: then list the five names).
//! NOTE: written without a Rust toolchain, not compiled or run yet.

use std::collections::BTreeSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tauri::{AppHandle, Manager};

const DIR: &str = "mlx-vault";
const BAK_EVERY_SECS: u64 = 600;

fn root(app: &AppHandle) -> Result<PathBuf, String> {
    let d = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join(DIR);
    fs::create_dir_all(&d).map_err(|e| e.to_string())?;
    Ok(d)
}

fn valid_key(key: &str) -> bool {
    key.starts_with("klapka.")
        && key.len() <= 160
        && !key.contains("..")
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
}

fn path_for(app: &AppHandle, key: &str) -> Result<PathBuf, String> {
    if !valid_key(key) {
        return Err("invalid key".into());
    }
    Ok(root(app)?.join(format!("{}.json", key)))
}

fn with_suffix(p: &Path, suffix: &str) -> PathBuf {
    let mut s = p.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

/// Every value this app stores is a JSON object; a cut-off file fails this cheap check.
fn looks_whole(s: &str) -> bool {
    let t = s.trim();
    !t.is_empty()
        && ((t.starts_with('{') && t.ends_with('}')) || (t.starts_with('[') && t.ends_with(']')))
}

fn read_good(p: &Path) -> Option<String> {
    for cand in [p.to_path_buf(), with_suffix(p, ".bak")] {
        if let Ok(s) = fs::read_to_string(&cand) {
            if looks_whole(&s) {
                return Some(s);
            }
        }
    }
    None
}

fn put_sync(p: &Path, value: &str) -> Result<(), String> {
    if !looks_whole(value) {
        return Err("refusing to store a value that is not a JSON object or array".into());
    }
    let tmp = with_suffix(p, ".tmp");
    {
        let mut f = fs::File::create(&tmp).map_err(|e| e.to_string())?;
        f.write_all(value.as_bytes()).map_err(|e| e.to_string())?;
        f.sync_all().map_err(|e| e.to_string())?;
    }
    if p.exists() {
        let bak = with_suffix(p, ".bak");
        let stale = fs::metadata(&bak)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .map(|e| e > Duration::from_secs(BAK_EVERY_SECS))
            .unwrap_or(true);
        if stale {
            if let Ok(cur) = fs::read_to_string(p) {
                if looks_whole(&cur) {
                    let _ = fs::copy(p, &bak);
                }
            }
        }
    }
    let mut last = String::new();
    for i in 0..5u64 {
        match fs::rename(&tmp, p) {
            Ok(()) => return Ok(()),
            Err(e) => {
                last = e.to_string();
                std::thread::sleep(Duration::from_millis(40 * (i + 1)));
            }
        }
    }
    Err(last)
}

#[tauri::command]
pub async fn vault_status(app: AppHandle) -> Result<String, String> {
    Ok(root(&app)?.to_string_lossy().into_owned())
}

#[tauri::command]
pub async fn vault_list(app: AppHandle) -> Result<Vec<String>, String> {
    let r = root(&app)?;
    let res = tauri::async_runtime::spawn_blocking(move || -> Result<Vec<String>, String> {
        let mut out = BTreeSet::new();
        for e in fs::read_dir(&r).map_err(|e| e.to_string())? {
            let name = e
                .map_err(|e| e.to_string())?
                .file_name()
                .to_string_lossy()
                .into_owned();
            let key = name
                .strip_suffix(".json")
                .or_else(|| name.strip_suffix(".json.bak"))
                .map(|s| s.to_string());
            if let Some(k) = key {
                if valid_key(&k) {
                    out.insert(k);
                }
            }
        }
        Ok(out.into_iter().collect())
    })
    .await
    .map_err(|e| e.to_string())?;
    res
}

#[tauri::command]
pub async fn vault_get(app: AppHandle, key: String) -> Result<Option<String>, String> {
    let p = path_for(&app, &key)?;
    tauri::async_runtime::spawn_blocking(move || -> Result<Option<String>, String> {
        Ok(read_good(&p))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn vault_put(app: AppHandle, key: String, value: String) -> Result<(), String> {
    let p = path_for(&app, &key)?;
    tauri::async_runtime::spawn_blocking(move || put_sync(&p, &value))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn vault_del(app: AppHandle, key: String) -> Result<(), String> {
    let p = path_for(&app, &key)?;
    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        for f in [p.clone(), with_suffix(&p, ".bak"), with_suffix(&p, ".tmp")] {
            match fs::remove_file(&f) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.to_string()),
            }
        }
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}
