// The client against a fake engine (fake-engine.mjs): the protocol, events, errors, a crash and its restart.
import { afterEach, expect, test } from "bun:test";
import { chmodSync, mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { DesktopSdk, DesktopSdkError, type Events } from "../src/index";

const engine = join(import.meta.dir, "fake-engine.mjs");
chmodSync(engine, 0o755);
const config = { apiUrl: "https://api.example", dataDir: "/tmp/recordem-test" };
let sdk: DesktopSdk | null = null;
afterEach(async () => { await sdk?.shutdown(); sdk = null; });

const next = <T extends keyof Events>(s: DesktopSdk, type: T) => new Promise<Events[T]>((r) => {
  const l = (p: Events[T]) => { s.removeEventListener(type, l); r(p); };
  s.addEventListener(type, l);
});

test("init, record, live lines, stop: commands answer and events arrive", async () => {
  sdk = new DesktopSdk({ enginePath: engine });
  const ready = await sdk.init(config);
  expect(ready.modelsReady).toBe(true);
  const window = await sdk.prepareDesktopAudioRecording();
  expect(window).toBe("w1");
  const started = next(sdk, "recording-started");
  const line = next(sdk, "realtime-event");
  expect(await sdk.startRecording({ windowId: window, uploadToken: "token" })).toBe("rec-1");
  expect((await started).recordingId).toBe("rec-1");
  expect((await line).data.name).toBe("Ana Souza");
  const done = next(sdk, "upload-status");
  await sdk.stopRecording({ windowId: window });
  expect((await done).status).toBe("done");
  expect(await sdk.listDevices()).toEqual(["MacBook Pro Microphone"]);
});

test("a refused command rejects with the engine's reason", async () => {
  sdk = new DesktopSdk({ enginePath: engine });
  await sdk.init(config);
  const e = await sdk.startRecording({ windowId: "w1" }).catch((e) => e);
  expect(e).toBeInstanceOf(DesktopSdkError);
  expect(e.message).toContain("needs an uploadToken");
  expect(e.command).toBe("startRecording");
});

test("an engine that exits on its own is started again, with the same init", async () => {
  const state = join(mkdtempSync(join(tmpdir(), "rd-")), "starts");
  sdk = new DesktopSdk({ enginePath: engine, env: { FAKE_STATE: state } });
  await sdk.init(config);
  const down = next(sdk, "shutdown");
  const back = next(sdk, "hello");
  await sdk.call("crash").catch(() => {});
  expect((await down).code).toBe(3);
  await back;
  // Re-initialised: commands work again on the new process.
  await new Promise((r) => setTimeout(r, 100));
  expect(await sdk.listDevices()).toEqual(["MacBook Pro Microphone"]);
  expect((await Bun.file(state).text()).trim().split("\n").length).toBe(2);
});

test("an engine of another protocol is refused", async () => {
  const s = new DesktopSdk({ enginePath: engine, env: { FAKE_PROTOCOL: "99" }, restart: false });
  const e = await s.init(config).catch((e) => e);
  expect(String(e.message)).toContain("protocol 99");
});

test("a missing engine says where it looked", async () => {
  const s = new DesktopSdk({ enginePath: "/nowhere/recordem-desktop" });
  const e = await s.init(config).catch((e) => e);
  expect(String(e.message)).toContain("/nowhere/recordem-desktop");
});
