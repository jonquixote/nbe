#!/usr/bin/env node
// A stub of R42's soak measure harness (packages/control-plane/src/r42-soak.measure.ts),
// to test scripts/soak-r42.sh's paths without an engine:
//   SOAK_HARNESS=scripts/soak-r42-stub.mjs SOAK_STUB_CASE=<case> scripts/soak-r42.sh
// It takes the harness's arguments (--out <dir> --minutes <n>) and writes a result.json
// with the fields the script reads, shaped as the harness's. Cases:
//   pass      every output measured and as long as its span (0.18 s short), 0 drops, 0 panics
//   short     the recording's video 2 s shorter than its span
//   null      the stream's video has no duration (ffprobe read none)
//   crash     exits 1 at once with a panic line, and no result.json
//   noresult  exits 0 with no result.json
//   hang      starts a stand-in engine in its own process group and never returns
//   escape    starts a stand-in engine in a session of its own (outside the show's
//             process group), then passes
// The stand-in engine is a copy of /bin/sleep named nbe-engine, in the show's folder.
import { chmodSync, copyFileSync, writeFileSync } from "node:fs";
import { spawn } from "node:child_process";
import { join } from "node:path";

const arg = (name) => {
  const i = process.argv.indexOf(name);
  return i < 0 ? undefined : process.argv[i + 1];
};
const out = arg("--out");
const minutes = Number(arg("--minutes") ?? "30");
const kase = process.env.SOAK_STUB_CASE ?? "pass";
if (!out) {
  console.error("--out <dir> is required");
  process.exit(2);
}
console.log(`STUB harness: case ${kase}, out ${out}, minutes ${minutes}`);

function standIn(detached) {
  const bin = join(out, "nbe-engine");
  copyFileSync("/bin/sleep", bin);
  chmodSync(bin, 0o755);
  const child = spawn(bin, ["600"], { detached, stdio: "ignore" });
  if (detached) child.unref();
  console.log(`STUB stand-in nbe-engine pid ${child.pid}${detached ? ", in a session of its own" : ""}`);
}

const wall = minutes * 60;
const result = {
  stub: kase,
  minutes,
  droppedFramesTotal: 0,
  panics: 0,
  panicLines: [],
  record: { wallSeconds: wall, video: { frames: wall * 30, seconds: wall - 0.18 }, audio: { frames: null, seconds: wall - 0.04 } },
  stream: { wallSeconds: wall, video: { frames: wall * 30, seconds: wall - 0.18 } },
};
switch (kase) {
  case "pass":
    break;
  case "short":
    result.record.video.seconds = wall - 2;
    break;
  case "null":
    result.stream.video.seconds = null;
    break;
  case "crash":
    console.error("thread 'main' panicked at the stub (STUB)");
    process.exit(1);
  case "noresult":
    process.exit(0);
  case "hang":
    standIn(false);
    setInterval(() => {}, 1 << 30);
    await new Promise(() => {});
    break;
  case "escape":
    standIn(true);
    break;
  default:
    console.error(`unknown SOAK_STUB_CASE ${kase}`);
    process.exit(2);
}
writeFileSync(join(out, "result.json"), JSON.stringify(result, null, 2));
console.log("STUB DONE");
