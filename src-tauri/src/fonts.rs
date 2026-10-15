// Lists the font families installed on this computer, for the script font picker (Script > Script font > Show more).
// Pure std, no new dependencies, read-only: it only reads the "name" table of the .ttf/.otf/.ttc files in the Windows font folders
// (system and per-user) and returns family names. The web side calls it with window.__TAURI_INTERNALS__.invoke('list_system_fonts').
// If this command is missing or fails, the app says so and lets the user type a font name instead.
//
// Hook it up in src-tauri/src/lib.rs (or main.rs):
//     mod fonts;
//     ... .invoke_handler(tauri::generate_handler![ /* your existing commands, */ fonts::list_system_fonts ])
// An app command needs no capability entry in Tauri 2 (capabilities only gate plugin and core commands).

use std::collections::BTreeSet;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

fn be16(b: &[u8], o: usize) -> Option<u16> {
    b.get(o..o + 2).map(|s| u16::from_be_bytes([s[0], s[1]]))
}

fn be32(b: &[u8], o: usize) -> Option<u32> {
    b.get(o..o + 4)
        .map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
}

fn read_at(f: &mut File, off: u64, len: usize) -> Option<Vec<u8>> {
    f.seek(SeekFrom::Start(off)).ok()?;
    let mut v = vec![0u8; len];
    f.read_exact(&mut v).ok()?;
    Some(v)
}

/// Family name of the font whose offset table starts at `base` (0 for .ttf/.otf, an entry of the header for .ttc).
/// Prefers the typographic family (name id 16, e.g. "Segoe UI") over the style-linked one (id 1, e.g. "Segoe UI Semibold"),
/// and English over other languages.
fn family_at(f: &mut File, base: u64) -> Option<String> {
    let head = read_at(f, base, 12)?;
    let n = be16(&head, 4)? as usize;
    if n == 0 || n > 200 {
        return None;
    }
    let dir = read_at(f, base + 12, n * 16)?;
    let mut table: Option<(u64, usize)> = None;
    for i in 0..n {
        let e = i * 16;
        let is_name = dir.get(e..e + 4).map_or(false, |t| t == &b"name"[..]);
        if is_name {
            table = Some((be32(&dir, e + 8)? as u64, be32(&dir, e + 12)? as usize));
            break;
        }
    }
    let (off, len) = table?;
    if len < 6 || len > 2_000_000 {
        return None;
    }
    let t = read_at(f, off, len)?;
    let count = be16(&t, 2)? as usize;
    let str_off = be16(&t, 4)? as usize;
    let mut best: Option<(u8, String)> = None;
    for i in 0..count {
        let r = 6 + i * 12;
        let (plat, enc, lang, id) = match (
            be16(&t, r),
            be16(&t, r + 2),
            be16(&t, r + 4),
            be16(&t, r + 6),
        ) {
            (Some(a), Some(b), Some(c), Some(d)) => (a, b, c, d),
            _ => break,
        };
        if (id != 1 && id != 16) || plat != 3 || (enc != 1 && enc != 10) {
            continue;
        }
        let l = be16(&t, r + 8)? as usize;
        let o = be16(&t, r + 10)? as usize;
        let raw = match t.get(str_off + o..str_off + o + l) {
            Some(x) => x,
            None => continue,
        };
        let units: Vec<u16> = raw
            .chunks_exact(2)
            .map(|c| u16::from_be_bytes([c[0], c[1]]))
            .collect();
        let name = match String::from_utf16(&units) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let rank = (if id == 16 { 0 } else { 2 }) + (if lang == 0x409 { 0 } else { 1 });
        if best.as_ref().map_or(true, |b| rank < b.0) {
            best = Some((rank, name));
        }
    }
    best.map(|b| b.1)
}

fn font_dirs() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(w) = std::env::var_os("WINDIR").or_else(|| std::env::var_os("SystemRoot")) {
        v.push(PathBuf::from(w).join("Fonts"));
    }
    if let Some(l) = std::env::var_os("LOCALAPPDATA") {
        v.push(
            PathBuf::from(l)
                .join("Microsoft")
                .join("Windows")
                .join("Fonts"),
        );
    }
    v
}

fn scan(dir: &Path, out: &mut BTreeSet<String>) {
    let rd = match std::fs::read_dir(dir) {
        Ok(r) => r,
        Err(_) => return,
    };
    for e in rd.flatten() {
        let p = e.path();
        let ext = p
            .extension()
            .and_then(|x| x.to_str())
            .map(|x| x.to_ascii_lowercase())
            .unwrap_or_default();
        if ext != "ttf" && ext != "otf" && ext != "ttc" && ext != "otc" {
            continue;
        }
        let mut f = match File::open(&p) {
            Ok(f) => f,
            Err(_) => continue,
        };
        let magic = match read_at(&mut f, 0, 12) {
            Some(m) => m,
            None => continue,
        };
        if &magic[0..4] == &b"ttcf"[..] {
            let n = be32(&magic, 8).unwrap_or(0).min(64) as u64;
            for i in 0..n {
                if let Some(o) = read_at(&mut f, 12 + i * 4, 4) {
                    let off = u32::from_be_bytes([o[0], o[1], o[2], o[3]]) as u64;
                    if let Some(name) = family_at(&mut f, off) {
                        out.insert(name);
                    }
                }
            }
        } else if let Some(name) = family_at(&mut f, 0) {
            out.insert(name);
        }
    }
}

/// Sorted, de-duplicated family names. Async so the scan runs off the main thread.
#[tauri::command]
pub async fn list_system_fonts() -> Vec<String> {
    let mut set = BTreeSet::new();
    for d in font_dirs() {
        scan(&d, &mut set);
    }
    set.into_iter()
        .filter(|s| !s.is_empty() && !s.starts_with('@'))
        .collect()
}
