//! Review4 step 6: the supervisor, with stub children (node one-liners and
//! `sleep`), no engine and no control plane.

import { test } from "node:test";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";

import { defaultBackoffMs, Supervisor, type ChildSpec, type SupervisorEvent } from "./supervisor.js";
import { tempDir } from "./test-tmp.js";

const NODE = process.execPath;
const sleep = (ms: number): Promise<void> => new Promise((r) => setTimeout(r, ms));
const alive = (pid: number): boolean => {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
};
async function until(check: () => boolean, ms: number): Promise<boolean> {
  const deadline = Date.now() + ms;
  while (Date.now() < deadline) {
    if (check()) return true;
    await sleep(20);
  }
  return check();
}
/** A child that runs until killed. */
const forever = (name: string, restart: boolean): ChildSpec => ({
  name,
  command: NODE,
  args: ["-e", "setInterval(() => {}, 1000)"],
  restart,
});

test("the backoff: at once, then 0.5 s doubling to 5 s", () => {
  assert.deepEqual([1, 2, 3, 4, 5, 6, 7].map(defaultBackoffMs), [0, 500, 1000, 2000, 4000, 5000, 5000]);
});

test("a killed engine is running again within 2 s", async (t) => {
  const events: SupervisorEvent[] = [];
  const sup = new Supervisor([forever("engine", true)], { logDir: tempDir("nbe-sup-"), caffeinate: false, onEvent: (e) => events.push(e) });
  t.after(() => sup.stop());
  await sup.start();
  const first = sup.pids()["engine"]!;
  process.kill(first, "SIGKILL");
  const back = await until(() => {
    const p = sup.pids()["engine"];
    return p !== undefined && p !== first && alive(p);
  }, 2000);
  assert.ok(back, `restarted within 2 s: ${JSON.stringify(events)}`);
  assert.ok(events.some((e) => e.kind === "restarting" && e.name === "engine" && e.afterMs === 0), "the first restart is at once");
});

test("the control plane is not restarted (until review4 step 7)", async (t) => {
  const events: SupervisorEvent[] = [];
  const sup = new Supervisor([forever("control-plane", false)], { logDir: tempDir("nbe-sup-"), caffeinate: false, onEvent: (e) => events.push(e) });
  t.after(() => sup.stop());
  await sup.start();
  const pid = sup.pids()["control-plane"]!;
  process.kill(pid, "SIGKILL");
  assert.ok(await until(() => events.some((e) => e.kind === "notRestarted"), 2000), "it says why it is not restarted");
  await sleep(700);
  assert.equal(sup.pids()["control-plane"], undefined, "still down");
  assert.equal(events.filter((e) => e.kind === "started").length, 1, "started once");
});

test("five failures within the window stop the retries", async (t) => {
  const events: SupervisorEvent[] = [];
  const crashing: ChildSpec = { name: "engine", command: NODE, args: ["-e", "process.exit(3)"], restart: true };
  const sup = new Supervisor([crashing], {
    logDir: tempDir("nbe-sup-"),
    caffeinate: false,
    backoffMs: () => 20,
    onEvent: (e) => events.push(e),
  });
  t.after(() => sup.stop());
  await sup.start();
  assert.ok(await until(() => events.some((e) => e.kind === "gaveUp"), 5000), `gave up: ${JSON.stringify(events.map((e) => e.kind))}`);
  await sleep(300);
  assert.equal(events.filter((e) => e.kind === "started").length, 5, "five starts, then no more");
});

test("stopping leaves no child and no grandchild: each child's process group goes", async (t) => {
  const dir = tempDir("nbe-sup-");
  // The child starts a grandchild (as the engine starts ffmpeg) and prints its pid.
  const parent: ChildSpec = {
    name: "engine",
    command: NODE,
    args: [
      "-e",
      "const c = require('child_process').spawn('sleep', ['30'], { stdio: 'ignore' }); console.log('grandchild ' + c.pid); setInterval(() => {}, 1000);",
    ],
    restart: true,
  };
  const sup = new Supervisor([parent], { logDir: dir, caffeinate: false });
  t.after(() => sup.stop());
  await sup.start();
  const pid = sup.pids()["engine"]!;
  let grandchild = 0;
  assert.ok(
    await until(() => {
      const m = /grandchild (\d+)/.exec(readFileSync(join(dir, "engine.log"), "utf8"));
      if (m) grandchild = Number(m[1]);
      return grandchild > 0;
    }, 3000),
    "the grandchild started",
  );
  assert.ok(alive(grandchild));
  await sup.stop();
  assert.ok(await until(() => !alive(pid) && !alive(grandchild), 3000), `child ${pid} and grandchild ${grandchild} are gone`);
  if (alive(grandchild)) process.kill(grandchild, "SIGKILL");
});

test("a child not ready in time fails the start by name", async (t) => {
  const sup = new Supervisor([{ ...forever("engine", true), ready: async () => false, readyMs: 300 }], {
    logDir: tempDir("nbe-sup-"),
    caffeinate: false,
  });
  t.after(() => sup.stop());
  await assert.rejects(sup.start(), /engine: not ready within 300 ms/);
});

test("each child's output is logged, rotated, and the old files are capped", async (t) => {
  const dir = tempDir("nbe-sup-");
  const chatty: ChildSpec = {
    name: "engine",
    command: NODE,
    // 40 lines of 200 bytes, one every 5 ms: separate writes, not one chunk.
    args: ["-e", "let i = 0; const t = setInterval(() => { process.stdout.write('x'.repeat(199) + '\\n'); if (++i === 40) clearInterval(t); }, 5); setInterval(() => {}, 1000);"],
    restart: true,
  };
  const sup = new Supervisor([chatty], { logDir: dir, caffeinate: false, rotateBytes: 1000, keepLogs: 3 });
  t.after(() => sup.stop());
  await sup.start();
  // Until the child has written its 8,000 bytes (a hang guard, not a measurement).
  const logs = (): string[] => readdirSync(dir).filter((f) => f.startsWith("engine")).sort();
  await until(() => logs().length === 4, 5000);
  await sup.stop();
  const files = logs();
  assert.deepEqual(files, ["engine.1.log", "engine.2.log", "engine.3.log", "engine.log"], `rotated and capped: ${files.join(", ")}`);
});

test("on macOS it keeps the Mac awake while it runs (caffeinate), and lets go when it stops", { skip: process.platform !== "darwin" }, async (t) => {
  const sup = new Supervisor([forever("engine", true)], { logDir: tempDir("nbe-sup-"), caffeinate: true });
  t.after(() => sup.stop());
  await sup.start();
  const pid = sup.caffeinatePid;
  assert.ok(pid !== undefined && alive(pid), "caffeinate is running");
  const ps = spawnSync("ps", ["-o", "command=", "-p", String(pid)], { encoding: "utf8" }).stdout;
  assert.match(ps, new RegExp(`caffeinate -dimsu -w ${process.pid}`), `held for the supervisor's pid: ${ps}`);
  await sup.stop();
  assert.ok(await until(() => !alive(pid!), 2000), "released on stop");
});

test("a child gets the base environment and its own variables, never the supervisor's secrets", async (t) => {
  const dir = tempDir("nbe-sup-");
  process.env.NBE_TOKENS_FOR_TEST = "admin-secret";
  t.after(() => delete process.env.NBE_TOKENS_FOR_TEST);
  const child: ChildSpec = {
    name: "engine",
    command: NODE,
    args: ["-e", "console.log('secret=' + process.env.NBE_TOKENS_FOR_TEST + ' own=' + process.env.NBE_OWN + ' path=' + (process.env.PATH ? 'set' : 'unset')); setInterval(() => {}, 1000);"],
    env: { NBE_OWN: "mine" },
    restart: true,
  };
  const sup = new Supervisor([child], { logDir: dir, caffeinate: false });
  t.after(() => sup.stop());
  await sup.start();
  let line = "";
  assert.ok(
    await until(() => {
      line = readFileSync(join(dir, "engine.log"), "utf8");
      return line.includes("secret=");
    }, 3000),
    "the child reported its environment",
  );
  assert.match(line, /secret=undefined/, `the supervisor's secret is not passed on: ${line}`);
  assert.match(line, /own=mine/, "its own variable is");
  assert.match(line, /path=set/, "and the base (PATH)");
});
