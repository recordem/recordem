// @recordem/desktop-sdk: record calls from your Electron or Node app. It starts recordem-desktop (the Recordem engine:
// call detection, capture, local transcription, upload), talks to it over stdin/stdout, and hands you its events. The
// engine only ever holds an upload token for one recording; your backend creates the recording with its own key.
//
//   import sdk from "@recordem/desktop-sdk";
//   await sdk.init({ apiUrl: "https://api.recordem.app", dataDir: app.getPath("userData") });
//   sdk.addEventListener("meeting-detected", async ({ window }) => {
//     const { uploadToken } = await yourBackend.createRecording();
//     await sdk.startRecording({ windowId: window.id, uploadToken });
//   });
import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { existsSync } from "node:fs";
import { dirname, join } from "node:path";
import { createInterface } from "node:readline";
import { fileURLToPath } from "node:url";
import {
  PROTOCOL, type EventType, type Events, type InitConfig, type Permission, type PermissionItem, type PendingRecording, type Ready,
  type StartRecording,
} from "./protocol";

export * from "./protocol";

export type Options = {
  /** The recordem-desktop executable. Default: $RECORDEM_DESKTOP_PATH, else the one this package downloaded. */
  enginePath?: string;
  /** Start the engine again when it exits on its own; default true. */
  restart?: boolean;
  /** How long a command may wait for its answer; default 30 s. */
  timeoutMs?: number;
  /** Environment for the engine, on top of this process's. */
  env?: Record<string, string>;
};

/** The engine refused a command, or didn't answer. */
export class DesktopSdkError extends Error {
  constructor(message: string, readonly command?: string) {
    super(message);
    this.name = "DesktopSdkError";
  }
}

type Listener<T extends EventType> = (payload: Events[T]) => void;

/** Where this package put the engine (scripts/install.mjs); in an Electron app, outside the asar archive. */
export function defaultEnginePath(): string {
  if (process.env.RECORDEM_DESKTOP_PATH) return process.env.RECORDEM_DESKTOP_PATH;
  const here = dirname(fileURLToPath(import.meta.url));
  return join(here, "..", "bin", "recordem-desktop").replace(/app\.asar([\\/])/, "app.asar.unpacked$1");
}

export class DesktopSdk {
  #opts: Options;
  #child: ChildProcessWithoutNullStreams | null = null;
  #ready: Promise<void> | null = null;
  #waiting = new Map<number, { resolve: (v: unknown) => void; reject: (e: Error) => void; timer: ReturnType<typeof setTimeout>; cmd: string }>();
  #next = 1;
  #listeners = new Map<EventType, Set<Listener<any>>>();
  #config: InitConfig | null = null;
  #closing = false;

  constructor(opts: Options = {}) {
    this.#opts = opts;
  }

  // --- events ------------------------------------------------------------------------------------------------------

  addEventListener<T extends EventType>(type: T, listener: Listener<T>): void {
    if (!this.#listeners.has(type)) this.#listeners.set(type, new Set());
    this.#listeners.get(type)!.add(listener);
  }

  removeEventListener<T extends EventType>(type: T, listener: Listener<T>): void {
    this.#listeners.get(type)?.delete(listener);
  }

  removeAllEventListeners(): void {
    this.#listeners.clear();
  }

  #emit<T extends EventType>(type: T, payload: Events[T]) {
    for (const l of this.#listeners.get(type) ?? []) {
      try { l(payload); } catch (e) { console.error(`recordem-desktop ${type} listener:`, e); }
    }
  }

  // --- the engine process -------------------------------------------------------------------------------------------

  #start(): Promise<void> {
    if (this.#ready) return this.#ready;
    const path = this.#opts.enginePath ?? defaultEnginePath();
    if (!existsSync(path)) return Promise.reject(new DesktopSdkError(`recordem-desktop not found at ${path} (set enginePath or RECORDEM_DESKTOP_PATH)`));
    const child = spawn(path, [], { stdio: ["pipe", "pipe", "pipe"], env: { ...process.env, ...this.#opts.env } });
    this.#child = child;
    this.#ready = new Promise<void>((resolve, reject) => {
      let hello = false;
      createInterface({ input: child.stdout }).on("line", (line) => {
        let m: any;
        try { m = JSON.parse(line); } catch { return console.error(`recordem-desktop: ${line}`); }
        if (typeof m.id === "number" && "ok" in m) {
          const w = this.#waiting.get(m.id);
          if (!w) return;
          this.#waiting.delete(m.id);
          clearTimeout(w.timer);
          if (m.ok) w.resolve(m.result ?? null);
          else w.reject(new DesktopSdkError(m.error ?? `${w.cmd} failed`, w.cmd));
          return;
        }
        if (typeof m.event === "string") {
          if (m.event === "hello" && !hello) {
            hello = true;
            if (m.payload?.protocol !== PROTOCOL) {
              reject(new DesktopSdkError(`recordem-desktop speaks protocol ${m.payload?.protocol}, this package ${PROTOCOL}`));
              child.kill();
              return;
            }
            resolve();
          }
          this.#emit(m.event as EventType, m.payload ?? {});
        }
      });
      createInterface({ input: child.stderr }).on("line", (line) => this.#emit("log", { level: "debug", message: line }));
      child.on("error", (e) => { if (!hello) reject(new DesktopSdkError(`recordem-desktop: ${e.message}`)); });
      child.on("exit", (code) => this.#exited(code));
    });
    return this.#ready;
  }

  #exited(code: number | null) {
    this.#child = null;
    this.#ready = null;
    for (const [, w] of this.#waiting) { clearTimeout(w.timer); w.reject(new DesktopSdkError("recordem-desktop exited", w.cmd)); }
    this.#waiting.clear();
    this.#emit("shutdown", { code });
    if (this.#closing || this.#opts.restart === false || !this.#config) return;
    // Gone on its own: again, with the same settings, after a pause (a crash at start doesn't spin).
    const config = this.#config;
    setTimeout(() => {
      this.#call("init", config).catch((e) => this.#emit("error", { type: "restart", message: String(e?.message ?? e) }));
    }, 1000);
  }

  async #call<T = unknown>(cmd: string, params: object = {}): Promise<T> {
    await this.#start();
    const id = this.#next++;
    return new Promise<T>((resolve, reject) => {
      const timer = setTimeout(() => {
        this.#waiting.delete(id);
        reject(new DesktopSdkError(`recordem-desktop didn't answer ${cmd}`, cmd));
      }, this.#opts.timeoutMs ?? 30_000);
      this.#waiting.set(id, { resolve: resolve as (v: unknown) => void, reject, timer, cmd });
      this.#child!.stdin.write(`${JSON.stringify({ id, cmd, params })}\n`);
    });
  }

  // --- commands ----------------------------------------------------------------------------------------------------

  /** Sends one command as it is: the raw form of every method below. */
  call<T = unknown>(cmd: string, params: object = {}): Promise<T> {
    return this.#call<T>(cmd, params);
  }

  /** Starts the engine (if it isn't running) and sets it up; recordings an earlier run left behind are resumed. */
  async init(config: InitConfig): Promise<Ready> {
    this.#closing = false;
    const ready = await this.#call<Ready>("init", config);
    this.#config = config;
    return ready;
  }

  /** Changes some settings; the next recording uses them. */
  async configure(config: Partial<InitConfig>): Promise<void> {
    await this.#call("configure", config);
    if (this.#config) this.#config = { ...this.#config, ...config };
  }

  /** Starts recording a window (a detected call, or one from `prepareDesktopAudioRecording`); returns the recording id. */
  async startRecording(params: StartRecording): Promise<string> {
    const r = await this.#call<{ recordingId: string }>("startRecording", params);
    return r.recordingId;
  }

  async stopRecording(params: { windowId: string }): Promise<void> {
    await this.#call("stopRecording", params);
  }

  /** Both channels are silence until `resumeRecording`; the recording keeps its clock. */
  async pauseRecording(params: { windowId: string }): Promise<void> {
    await this.#call("pauseRecording", params);
  }

  async resumeRecording(params: { windowId: string }): Promise<void> {
    await this.#call("resumeRecording", params);
  }

  /** The token for a recording started without one, or a fresh one for a recording whose token expired. */
  async setUploadToken(params: { recordingId: string; uploadToken: string }): Promise<void> {
    await this.#call("setUploadToken", params);
  }

  /** A window to record with no call (in person); returns its id. */
  async prepareDesktopAudioRecording(): Promise<string> {
    const r = await this.#call<{ windowId: string }>("prepareDesktopAudioRecording");
    return r.windowId;
  }

  async permissions(): Promise<PermissionItem[]> {
    return this.#call<PermissionItem[]>("permissions");
  }

  /** Shows macOS's prompt (it waits for the person); once refused, nothing: then `openPermissionSettings`. */
  async requestPermission(permission: Permission): Promise<void> {
    await this.#call("requestPermission", { permission });
  }

  async openPermissionSettings(permission: Permission): Promise<void> {
    await this.#call("openPermissionSettings", { permission });
  }

  /** The recording's microphone is silence while muted. */
  async setMuted(muted: boolean): Promise<void> {
    await this.#call("setMuted", { muted });
  }

  /** Microphones by name. */
  async listDevices(): Promise<string[]> {
    return this.#call<string[]>("listDevices");
  }

  /** Downloads the speech models (~575 MB, once); progress comes as `model-download` events. */
  async downloadModels(): Promise<void> {
    await this.#call("downloadModels");
  }

  async pending(): Promise<PendingRecording[]> {
    return this.#call<PendingRecording[]>("pending");
  }

  /** Stops the engine: a recording going on is stopped and sent first, then it exits. */
  async shutdown(): Promise<void> {
    if (!this.#child) return;
    this.#closing = true;
    const child = this.#child;
    const gone = new Promise<void>((r) => child.once("exit", () => r()));
    await this.#call("shutdown").catch(() => {});
    await gone;
  }
}

/** One engine for the app, as most apps want. */
const sdk = new DesktopSdk();
export default sdk;
