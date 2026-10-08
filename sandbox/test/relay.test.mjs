import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Readable, Writable } from "node:stream";
import { after, before, test } from "node:test";
import { fileURLToPath } from "node:url";
import { readFile as readBridgeFile, resolveReply } from "../dist/bridge.js";
import { relay } from "../dist/relay.js";

const container = fileURLToPath(new URL("../container/", import.meta.url));
const sent = [];
const waiters = [];
const write = process.stdout.write.bind(process.stdout);
let cwd;

before(async () => {
  cwd = await mkdtemp(join(tmpdir(), "reseam-relay-"));
  process.stdout.write = (chunk, ...rest) => {
    const text = chunk.toString();
    if (!text.startsWith("{")) return write(chunk, ...rest);
    for (const line of text.trim().split("\n")) {
      const message = JSON.parse(line);
      const waiter = waiters.findIndex((entry) => entry.type === message.type);
      if (waiter === -1) sent.push(message);
      else waiters.splice(waiter, 1)[0].resolve(message);
    }
    return true;
  };
});

after(async () => {
  process.stdout.write = write;
  await rm(cwd, { recursive: true, force: true });
});

function next(type) {
  const index = sent.findIndex((message) => message.type === type);
  if (index !== -1) return Promise.resolve(sent.splice(index, 1)[0]);
  return new Promise((resolve) => waiters.push({ type, resolve }));
}

function run(command, { timeout_ms = 10000, signal = new AbortController().signal } = {}) {
  const child = spawn("python3", [join(container, "relay.py")], { stdio: ["pipe", "pipe", "inherit"] });
  const process = {
    stdin: Writable.toWeb(child.stdin),
    stdout: Readable.toWeb(child.stdout).pipeThrough(new TextDecoderStream()),
  };
  return relay(process, 7, { command, cwd, path: join(container, "bin"), timeout_ms }, signal);
}

test("bridge calls read sandbox files and write returned files", async () => {
  await writeFile(join(cwd, "out.bin"), Buffer.from([0, 255, 10]));
  const result = run("echo before; discord send --file out.bin; echo $?");
  const call = await next("call");
  assert.equal(call.sandbox, 7);
  assert.equal(call.command, "discord");
  assert.deepEqual(call.args, ["send", "--file", "out.bin"]);
  void readBridgeFile({ type: "read_file", id: call.id, path: "out.bin", max_bytes: 3 });
  assert.deepEqual(Buffer.from((await next("file_result")).result.base64, "base64"), Buffer.from([0, 255, 10]));
  void readBridgeFile({ type: "read_file", id: call.id, path: "out.bin", max_bytes: 2 });
  assert.match((await next("file_result")).result.error, /limit/);
  resolveReply(call.id, { command: { stdout: "sent\n", stderr: "", exit_code: 0,
    file: { path: "saved.txt", base64: Buffer.from("saved").toString("base64") } } });
  const output = await result;
  assert.equal(output.stdout, "before\nsent\n0\n");
  assert.equal(output.exit_code, 0);
  assert.equal(await readFile(join(cwd, "saved.txt"), "utf8"), "saved");
});

test("fetch sends method, headers, and stdin body, and fails on HTTP errors", async () => {
  const result = run("printf payload | fetch -X post -H 'X-Test: yes' --body - https://example.com/api");
  const fetch = await next("fetch");
  assert.equal(fetch.method, "POST");
  assert.deepEqual(fetch.headers, { "x-test": "yes" });
  assert.equal(Buffer.from(fetch.body_base64, "base64").toString(), "payload");
  resolveReply(fetch.id, { fetch: { status: 404, status_text: "Not Found", headers: {},
    body_base64: Buffer.from("nope").toString("base64"), url: fetch.url } });
  const output = await result;
  assert.equal(output.stdout, "nope");
  assert.equal(output.stderr, "fetch: HTTP 404 Not Found\n");
  assert.equal(output.exit_code, 22);
});

test("view attaches images to the result", async () => {
  const output = await run("printf image > a.png && view a.png");
  assert.equal(output.stdout, "Attached a.png to this result.\n");
  assert.deepEqual(output.images, [{ name: "a.png", base64: Buffer.from("image").toString("base64") }]);
});

test("timeouts and cancellation kill the command", async () => {
  const timedOut = await run("sleep 5", { timeout_ms: 300 });
  assert.equal(timedOut.exit_code, 137);
  assert.match(timedOut.stderr, /timed out after 0.3s/);
  const controller = new AbortController();
  const started = Date.now();
  const cancelled = run("sleep 5", { signal: controller.signal });
  setTimeout(() => controller.abort(), 300);
  assert.match((await cancelled).stderr, /cancelled/);
  assert.ok(Date.now() - started < 4000);
});

test("background processes end with the command and long output is truncated", async () => {
  const started = Date.now();
  const output = await run("sleep 100 & head -c 2000000 /dev/zero | tr '\\0' a");
  assert.ok(Date.now() - started < 4000);
  assert.match(output.stdout, /\[output truncated: 2000000 bytes in total\]\n$/);
});
