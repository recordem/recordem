#!/usr/bin/env node
// A stand-in for recordem-desktop that speaks its protocol: enough to test the client without a Mac.
//   FAKE_PROTOCOL=<n>     the protocol it claims
//   FAKE_STATE=<file>     counts its starts there (to see a restart)
import { appendFileSync, readFileSync } from "node:fs";
import { createInterface } from "node:readline";

const out = (o) => process.stdout.write(`${JSON.stringify(o)}\n`);
if (process.env.FAKE_STATE) appendFileSync(process.env.FAKE_STATE, "start\n");
const starts = process.env.FAKE_STATE ? readFileSync(process.env.FAKE_STATE, "utf8").split("\n").filter(Boolean).length : 1;
out({ event: "hello", payload: { protocol: Number(process.env.FAKE_PROTOCOL ?? 1), version: "0.0.0-fake" } });
process.stderr.write("fake engine up\n");

let windows = 0;
createInterface({ input: process.stdin }).on("line", (line) => {
  const { id, cmd, params } = JSON.parse(line);
  const ok = (result) => out({ id, ok: true, ...(result === undefined ? {} : { result }) });
  const fail = (error) => out({ id, ok: false, error });
  switch (cmd) {
    case "init":
      if (!params.apiUrl || !params.dataDir) return fail("init needs apiUrl and dataDir");
      return ok({ protocol: 1, version: "0.0.0-fake", modelsReady: true, pending: [], starts });
    case "prepareDesktopAudioRecording":
      return ok({ windowId: `w${++windows}` });
    case "startRecording": {
      if (!params.uploadToken && !params.recordingId) return fail("startRecording needs an uploadToken");
      const recordingId = params.recordingId ?? "rec-1";
      ok({ recordingId });
      out({ event: "recording-started", payload: { window: { id: params.windowId }, recordingId } });
      out({ event: "realtime-event", payload: { window: { id: params.windowId }, recordingId, event: "transcript.data",
        data: { start_ms: 0, end_ms: 900, channel: "system", speaker: "system:1", name: "Ana Souza", text: "Oi, tudo bem?" } } });
      return;
    }
    case "stopRecording":
      ok();
      out({ event: "recording-ended", payload: { window: { id: params.windowId }, recordingId: "rec-1" } });
      out({ event: "upload-status", payload: { recordingId: "rec-1", status: "done", progress: 1000 } });
      return;
    case "listDevices":
      return ok(["MacBook Pro Microphone"]);
    case "crash":
      return process.exit(3);
    case "shutdown":
      ok();
      out({ event: "shutdown", payload: { code: 0 } });
      return setTimeout(() => process.exit(0), 20);
    default:
      return fail(`unknown command ${cmd}`);
  }
});
