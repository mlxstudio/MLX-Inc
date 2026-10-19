use std::{
    collections::HashMap,
    fs,
    fs::OpenOptions,
    io::Write,
    path::{Component, Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Manager};
use tauri_plugin_dialog::DialogExt;

#[derive(Deserialize)]
pub struct FolderFile {
    path: String,
    text: Option<String>,
    data: Option<String>,
    bin: Option<bool>,
}

#[derive(Serialize)]
pub struct FolderEntry {
    rel: String,
    dir: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    base64: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    mime: Option<String>,
}

#[derive(Serialize)]
pub struct ProjectFolder {
    path: String,
    name: String,
}

#[derive(Serialize)]
pub struct ProjectFolderLink {
    project_id: String,
    name: String,
}

#[derive(Serialize)]
pub struct FolderContents {
    name: String,
    entries: Vec<FolderEntry>,
}

fn safe_rel(path: &str) -> Result<PathBuf, String> {
    let path = Path::new(path);
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err("invalid relative folder path".into());
    }
    Ok(path.to_path_buf())
}

fn ensure_dirs(root: &Path, relative: &Path) -> Result<(), String> {
    let mut current = root.to_path_buf();
    for part in relative.components() {
        let Component::Normal(name) = part else {
            return Err("invalid relative folder path".into());
        };
        current.push(name);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err("project folder contains an unsafe path".into())
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&current).map_err(|e| e.to_string())?;
            }
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok(())
}

fn folder_name(title: &str) -> String {
    let name: String = title
        .chars()
        .map(|c| {
            if c.is_control() || "<>:\"/\\|?*".contains(c) {
                ' '
            } else {
                c
            }
        })
        .collect();
    let name = name.trim().trim_end_matches([' ', '.']).trim();
    let reserved = matches!(
        name.split('.')
            .next()
            .unwrap_or("")
            .to_ascii_uppercase()
            .as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    );
    if name.is_empty() || matches!(name, "." | "..") || reserved {
        "Project".into()
    } else {
        name.chars().take(80).collect()
    }
}

fn manifest_project_id(path: &Path) -> Option<String> {
    let value: Value = serde_json::from_slice(&fs::read(path.join("project.json")).ok()?).ok()?;
    value.get("project")?.get("id")?.as_str().map(str::to_owned)
}

fn links_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("mlx-project-folders");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.join("links.json"))
}

fn read_links(app: &AppHandle) -> Result<HashMap<String, String>, String> {
    let path = links_path(app)?;
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| e.to_string()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(HashMap::new()),
        Err(error) => Err(error.to_string()),
    }
}

fn write_link(app: &AppHandle, project_id: &str, folder: &Path) -> Result<(), String> {
    if project_id.is_empty()
        || project_id.len() > 160
        || !project_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err("invalid project id".into());
    }
    let mut links = read_links(app)?;
    links.insert(
        project_id.to_string(),
        folder.to_string_lossy().into_owned(),
    );
    let path = links_path(app)?;
    let bytes = serde_json::to_vec(&links).map_err(|e| e.to_string())?;
    write_atomic(&path, &bytes)
}

fn linked_folder(app: &AppHandle, project_id: &str) -> Result<PathBuf, String> {
    let folder = read_links(app)?
        .remove(project_id)
        .ok_or_else(|| "no user-selected folder is linked to this project".to_string())?;
    let folder = PathBuf::from(folder);
    if !folder.is_absolute() || !folder.is_dir() {
        return Err("the linked project folder is unavailable".into());
    }
    folder.canonicalize().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn list_project_folders(app: AppHandle) -> Result<Vec<ProjectFolderLink>, String> {
    Ok(read_links(&app)?
        .into_iter()
        .map(|(project_id, path)| ProjectFolderLink {
            project_id,
            name: Path::new(&path)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Project".into()),
        })
        .collect())
}

fn prepare_project_folder(
    parent: String,
    project_id: String,
    title: String,
) -> Result<ProjectFolder, String> {
    let parent = PathBuf::from(parent);
    if !parent.is_absolute() || !parent.is_dir() {
        return Err("the selected parent folder is unavailable".into());
    }
    let base = folder_name(&title);
    for index in 1..=1000 {
        let name = if index == 1 {
            base.clone()
        } else {
            format!("{base} ({index})")
        };
        let path = parent.join(&name);
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&path).map_err(|e| format!("cannot create project folder: {e}"))?;
                return Ok(ProjectFolder {
                    path: path.to_string_lossy().into_owned(),
                    name,
                });
            }
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => continue,
            Ok(_) => {}
            Err(error) => return Err(error.to_string()),
        }
        let empty = fs::read_dir(&path)
            .map_err(|e| e.to_string())?
            .next()
            .is_none();
        if empty || manifest_project_id(&path) == Some(project_id.clone()) {
            return Ok(ProjectFolder {
                path: path.to_string_lossy().into_owned(),
                name,
            });
        }
    }
    Err("could not find an available project folder name".into())
}

#[tauri::command]
pub async fn choose_project_folder(
    app: AppHandle,
    project_id: String,
    title: String,
) -> Result<Option<String>, String> {
    let Some(folder) = app.dialog().file().blocking_pick_folder() else {
        return Ok(None);
    };
    let parent = folder
        .into_path()
        .map_err(|e| e.to_string())?
        .to_string_lossy()
        .into_owned();
    let folder = prepare_project_folder(parent, project_id.clone(), title)?;
    write_link(&app, &project_id, Path::new(&folder.path))?;
    Ok(Some(folder.name))
}

fn file_bytes(file: &FolderFile) -> Result<Vec<u8>, String> {
    if file.bin.unwrap_or(false) {
        let data = file
            .data
            .as_deref()
            .ok_or_else(|| "binary project file has no data".to_string())?;
        let payload = data
            .split_once(";base64,")
            .map(|(_, encoded)| encoded)
            .ok_or_else(|| "unsupported binary project file encoding".to_string())?;
        STANDARD
            .decode(payload)
            .map_err(|e| format!("invalid binary project file: {e}"))
    } else {
        file.text
            .as_ref()
            .map(|text| text.as_bytes().to_vec())
            .ok_or_else(|| "text project file has no contents".to_string())
    }
}

fn keep_old(root: &Path, name: &str, bytes: &[u8]) -> Result<(), String> {
    let removed = root.join("_removed");
    ensure_dirs(root, Path::new("_removed"))?;
    let leaf = Path::new(name)
        .file_name()
        .ok_or_else(|| "invalid backup file name".to_string())?
        .to_string_lossy();
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let mut target = removed.join(leaf.as_ref());
    if target.exists() {
        target = removed.join(format!("{stamp} {leaf}"));
    }
    write_atomic(&target, bytes)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let leaf = path
        .file_name()
        .ok_or_else(|| "invalid project file path".to_string())?
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

fn write_folder(
    root: &Path,
    files: Vec<FolderFile>,
    dirs: Vec<String>,
    since: u64,
) -> Result<usize, String> {
    if !root.is_absolute() || !root.is_dir() {
        return Err("the linked project folder is unavailable".into());
    }
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let mut current = std::collections::HashSet::new();
    let previous_files = fs::read(root.join("project.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|value| value.get("files").and_then(Value::as_object).cloned());
    for dir in &dirs {
        let rel = safe_rel(dir)?;
        ensure_dirs(&root, &rel)?;
    }
    let mut written = 0;
    for file in &files {
        let rel = safe_rel(&file.path)?;
        current.insert(file.path.replace('\\', "/"));
        let path = root.join(rel);
        let parent = path
            .parent()
            .ok_or_else(|| "invalid project file path".to_string())?;
        let parent_rel = parent.strip_prefix(&root).map_err(|e| e.to_string())?;
        ensure_dirs(&root, parent_rel)?;
        if fs::symlink_metadata(&path)
            .map(|metadata| metadata.file_type().is_symlink())
            .unwrap_or(false)
        {
            return Err("project folder contains an unsafe file path".into());
        }
        let bytes = file_bytes(file)?;
        if let Ok(existing) = fs::read(&path) {
            if existing == bytes {
                continue;
            }
            let modified = fs::metadata(&path)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|m| m.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            if modified > since.saturating_add(2000)
                && file.path != "project.json"
                && file.path != "README.txt"
            {
                keep_old(&root, &file.path, &existing)?;
            }
        }
        write_atomic(&path, &bytes).map_err(|e| format!("cannot save {}: {e}", file.path))?;
        written += 1;
    }

    if let Some(old_files) = previous_files {
        for old_path in old_files.keys() {
            if current.contains(old_path) {
                continue;
            }
            let rel = safe_rel(old_path)?;
            let path = root.join(rel);
            let parent = path
                .parent()
                .ok_or_else(|| "invalid project file path".to_string())?;
            let parent_rel = parent.strip_prefix(&root).map_err(|e| e.to_string())?;
            if ensure_dirs(&root, parent_rel).is_err() {
                continue;
            }
            if fs::symlink_metadata(&path)
                .map(|metadata| metadata.file_type().is_symlink())
                .unwrap_or(false)
            {
                continue;
            }
            if let Ok(bytes) = fs::read(&path) {
                keep_old(&root, old_path, &bytes)?;
                fs::remove_file(path).map_err(|e| e.to_string())?;
            }
        }
    }
    Ok(written)
}

#[tauri::command]
pub async fn write_project_folder(
    app: AppHandle,
    project_id: String,
    files: Vec<FolderFile>,
    dirs: Vec<String>,
    since: u64,
) -> Result<usize, String> {
    let path = linked_folder(&app, &project_id)?;
    tauri::async_runtime::spawn_blocking(move || write_folder(&path, files, dirs, since))
        .await
        .map_err(|e| e.to_string())?
}

fn walk_folder(root: &Path, dir: &Path, out: &mut Vec<FolderEntry>) -> Result<(), String> {
    let entries = fs::read_dir(dir).map_err(|e| e.to_string())?;
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        let rel = path
            .strip_prefix(root)
            .map_err(|e| e.to_string())?
            .to_string_lossy()
            .replace('\\', "/");
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || name == "_removed" {
            continue;
        }
        let kind = entry.file_type().map_err(|e| e.to_string())?;
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            out.push(FolderEntry {
                rel: format!("{rel}/"),
                dir: true,
                text: None,
                base64: None,
                size: None,
                mime: None,
            });
            walk_folder(root, &path, out)?;
            continue;
        }
        if !kind.is_file() {
            continue;
        }
        let metadata = entry.metadata().map_err(|e| e.to_string())?;
        let size = metadata.len();
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        let text_file = matches!(
            ext.to_ascii_lowercase().as_str(),
            "script" | "synopsis" | "breakdown" | "fountain" | "fdx" | "txt" | "md" | "json"
        );
        let mime = match ext.to_ascii_lowercase().as_str() {
            "png" => "image/png",
            "jpg" | "jpeg" => "image/jpeg",
            "gif" => "image/gif",
            "webp" => "image/webp",
            "pdf" => "application/pdf",
            _ => "application/octet-stream",
        };
        if size > 60_000_000 {
            out.push(FolderEntry {
                rel,
                dir: false,
                text: None,
                base64: None,
                size: Some(size),
                mime: Some(mime.into()),
            });
            continue;
        }
        if text_file {
            let data = fs::read(path).map_err(|e| format!("cannot read {rel}: {e}"))?;
            let (text, base64) = match String::from_utf8(data) {
                Ok(text) => (Some(text), None),
                Err(error) => (None, Some(STANDARD.encode(error.into_bytes()))),
            };
            out.push(FolderEntry {
                rel,
                dir: false,
                text,
                base64,
                size: Some(size),
                mime: Some(mime.into()),
            });
        } else {
            let data = fs::read(path).map_err(|e| format!("cannot read {rel}: {e}"))?;
            out.push(FolderEntry {
                rel,
                dir: false,
                text: None,
                base64: Some(STANDARD.encode(data)),
                size: Some(size),
                mime: Some(mime.into()),
            });
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn open_project_folder(
    app: AppHandle,
    project_id: String,
) -> Result<Option<FolderContents>, String> {
    let Some(folder) = app.dialog().file().blocking_pick_folder() else {
        return Ok(None);
    };
    let path = folder
        .into_path()
        .map_err(|e| e.to_string())?
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Project".into());
    tauri::async_runtime::spawn_blocking(move || {
        let mut entries = Vec::new();
        walk_folder(&path, &path, &mut entries)?;
        write_link(&app, &project_id, &path)?;
        Ok(Some(FolderContents { name, entries }))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_folder() -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("mlx-ink-folder-test-{stamp}"));
        fs::create_dir(&path).unwrap();
        path
    }

    #[test]
    fn relative_paths_reject_traversal() {
        assert!(safe_rel("scripts/main.script").is_ok());
        assert!(safe_rel("../outside.txt").is_err());
        assert!(safe_rel("").is_err());
    }

    #[test]
    fn folder_sync_atomically_saves_and_preserves_removed_files() {
        let root = temp_folder();
        let manifest = r#"{"files":{"main.script":{"type":"script"}}}"#;
        write_folder(
            &root,
            vec![
                FolderFile {
                    path: "main.script".into(),
                    text: Some("first".into()),
                    data: None,
                    bin: Some(false),
                },
                FolderFile {
                    path: "project.json".into(),
                    text: Some(manifest.into()),
                    data: None,
                    bin: Some(false),
                },
            ],
            Vec::new(),
            0,
        )
        .unwrap();
        write_folder(
            &root,
            vec![FolderFile {
                path: "project.json".into(),
                text: Some(r#"{"files":{}}"#.into()),
                data: None,
                bin: Some(false),
            }],
            Vec::new(),
            0,
        )
        .unwrap();

        assert!(!root.join("main.script").exists());
        assert_eq!(
            fs::read_to_string(root.join("_removed").join("main.script")).unwrap(),
            "first"
        );
        assert_eq!(
            fs::read_to_string(root.join("project.json")).unwrap(),
            r#"{"files":{}}"#
        );
        fs::remove_dir_all(root).unwrap();
    }
}
