/**
 * MLX Ink – desktop bridge (Tauri 2).
 *
 * The main application lives in index.html. This module adds what a packaged desktop app needs
 * and does nothing at all when the page is opened in a normal browser:
 *
 *  1. window.mlxDesktop.saveText()  – native "Save as" dialog for every export (browser downloads
 *     are unreliable inside a WebView).
 *  2. window.mlxDesktop.checkForUpdates() – checks for signed GitHub releases and silently installs
 *     them when available.
 *  3. A write-through mirror of the app's localStorage data (keys "klapka.*") to real files in the
 *     app-data folder, so scripts survive a cleared/reset WebView profile.
 *     Since schema 3 the app has its own disk store (Vault in index.html, Rust vault_* commands in
 *     src-tauri/src/vault.rs, folder mlx-vault) which is the PRIMARY copy for projects: it has no
 *     size limit. This mirror stays as a second, independent safety net for every key that fits
 *     in localStorage; a key that does not fit is simply not mirrored here.
 *  4. A safe shutdown: the app's own save handler runs, then both this mirror and the app's disk
 *     queue (window.mlxInkFlush) are flushed before the window closes.
 */
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { check } from "@tauri-apps/plugin-updater";

type UpdateCheckResult = "up-to-date" | "installed";

interface MlxDesktop {
  /** Shows the native Save dialog. Resolves to the saved file's name, or null if cancelled. */
  saveText(name: string, text: string): Promise<string | null>;
  checkForUpdates(): Promise<UpdateCheckResult>;
}

declare global {
  interface Window {
    mlxDesktop?: MlxDesktop;
    /** Set by index.html (Vault): resolves when its queued disk writes are done (or after a short wait). */
    mlxInkFlush?: () => Promise<unknown>;
  }
}

const KEY_PREFIX = "klapka.";
const PROJECT_PREFIXES = ["klapka.p.", "klapka.proj."]; // old one-script records and schema 3 projects
const INDEX_KEY = "klapka.idx";
const UI_KEY = "klapka.ui";
const RELOAD_GUARD = "mlxink.restored";
const FLUSH_DELAY_MS = 400;
const RETRY_DELAY_MS = 5000;
const CLOSE_FLUSH_TIMEOUT_MS = 3500;

// Same check the Tauri API itself uses internally.
const inTauri = "__TAURI_INTERNALS__" in window;

async function checkForUpdates(): Promise<UpdateCheckResult> {
  const update = await check();
  if (!update) return "up-to-date";

  try {
    window.dispatchEvent(new Event("mlxink:update-downloading"));
    await update.downloadAndInstall();
    return "installed";
  } finally {
    await update.close();
  }
}

/* ------------------------------------------------------------------ disk mirror */

const queue = new Map<string, string | null>(); // key -> value to write, or null to delete
let flushTimer: number | undefined;
let inFlight: Promise<void> = Promise.resolve();

const nativeSetItem = Storage.prototype.setItem;
const nativeRemoveItem = Storage.prototype.removeItem;

function enqueue(key: string, value: string | null): void {
  queue.set(key, value);
  window.clearTimeout(flushTimer);
  flushTimer = window.setTimeout(() => void flush(), FLUSH_DELAY_MS);
}

function flush(): Promise<void> {
  window.clearTimeout(flushTimer);
  inFlight = inFlight.then(async () => {
    while (queue.size > 0) {
      const [key, value] = queue.entries().next().value as [string, string | null];
      queue.delete(key);
      try {
        if (value === null) await invoke("store_remove", { key });
        else await invoke("store_set", { key, value });
      } catch (err) {
        console.error("MLX Ink: could not mirror", key, "to disk; will retry", err);
        if (!queue.has(key)) queue.set(key, value); // keep, unless a newer value arrived meanwhile
        flushTimer = window.setTimeout(() => void flush(), RETRY_DELAY_MS);
        return;
      }
    }
  });
  return inFlight;
}

function installStorageHooks(): void {
  Storage.prototype.setItem = function (this: Storage, key: string, value: string): void {
    nativeSetItem.call(this, key, value); // throws on quota exceeded: then nothing is mirrored
    if (this === window.localStorage && key.startsWith(KEY_PREFIX)) enqueue(key, String(value));
  };
  Storage.prototype.removeItem = function (this: Storage, key: string): void {
    nativeRemoveItem.call(this, key);
    if (this === window.localStorage && key.startsWith(KEY_PREFIX)) enqueue(key, null);
  };
}

function localKeys(): string[] {
  const keys: string[] = [];
  for (let i = 0; i < localStorage.length; i++) {
    const k = localStorage.key(i);
    if (k && k.startsWith(KEY_PREFIX)) keys.push(k);
  }
  return keys;
}

/**
 * Reconcile localStorage with the files on disk. Rules (never destructive):
 *  - a key present on disk but missing from localStorage is restored (e.g. WebView data was reset);
 *  - if localStorage holds no scripts at all, the saved index and UI preferences are restored too;
 *  - a key whose value differs is NOT overwritten from disk: localStorage is the live copy;
 *  - anything in localStorage that disk lacks or has an older copy of is written out.
 */
async function reconcile(): Promise<void> {
  let disk: Record<string, string>;
  try {
    disk = await invoke<Record<string, string>>("store_load_all");
  } catch (err) {
    console.error("MLX Ink: could not read the data folder", err);
    return;
  }

  const fresh = !localKeys().some((k) => PROJECT_PREFIXES.some((p) => k.startsWith(p)));
  let restored = 0;
  for (const [key, value] of Object.entries(disk)) {
    const missing = localStorage.getItem(key) === null;
    const overwrite = fresh && (key === INDEX_KEY || key === UI_KEY);
    if (!missing && !overwrite) continue;
    try {
      nativeSetItem.call(localStorage, key, value); // bypass the hook: it is already on disk
      restored++;
    } catch (err) {
      console.error("MLX Ink: could not restore", key, err);
    }
  }

  // The page has already started by now; reload once so it boots with the restored data.
  if (restored > 0 && !sessionStorage.getItem(RELOAD_GUARD)) {
    sessionStorage.setItem(RELOAD_GUARD, "1");
    location.reload();
    return;
  }

  for (const key of localKeys()) {
    const value = localStorage.getItem(key);
    if (value !== null && disk[key] !== value) enqueue(key, value);
  }
}

async function startMirror(): Promise<void> {
  installStorageHooks(); // first, so nothing written from now on is missed
  await reconcile();
  await installCloseHandler();
}

/* ------------------------------------------------------------------ safe shutdown */

async function installCloseHandler(): Promise<void> {
  const win = getCurrentWindow();
  await win.onCloseRequested(async (event) => {
    event.preventDefault();
    try {
      // The app saves on "pagehide" (commit + store); its normal autosave is debounced by 700 ms.
      window.dispatchEvent(new Event("pagehide"));
      const appFlush = window.mlxInkFlush ? window.mlxInkFlush().catch(() => undefined) : Promise.resolve();
      await Promise.race([
        Promise.all([flush(), appFlush]),
        new Promise<void>((r) => window.setTimeout(r, CLOSE_FLUSH_TIMEOUT_MS)),
      ]);
    } finally {
      await win.destroy(); // always close, even if saving failed
    }
  });
}

/* ------------------------------------------------------------------ start
   Kept at the very end on purpose: everything above (notably the captured native
   Storage methods) must be initialised before the hooks are installed. */

if (inTauri) {
  window.mlxDesktop = {
    async saveText(name, text) {
      const path = await invoke<string | null>("export_text", { name, contents: text });
      if (!path) return null;
      return path.split(/[\\/]/).pop() || path;
    },
    checkForUpdates,
  };
  void startMirror();
  window.setTimeout(() => {
    void checkForUpdates().catch((err) => {
      console.error("MLX Ink: update check failed", err);
      window.dispatchEvent(new Event("mlxink:update-error"));
    });
  }, 5000);
}
