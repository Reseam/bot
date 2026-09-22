import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdtemp, mkdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createInterface } from "node:readline";
import test from "node:test";

for (const team of [false, true]) {
  for (const command of ["discord", "archive"]) {
    test(`${command} file transfers use the active ${team ? "team" : "member"} filesystem`, { timeout: 15000 }, async () => {
      const root = await mkdtemp(join(tmpdir(), "reseam-files-"));
      const workspace = join(root, "workspace");
      const repos = join(root, "repos");
      await mkdir(workspace);
      await mkdir(repos);
      const binary = Buffer.from([0, 255, 128, 13, 10, 42]);
      await writeFile(join(workspace, "binary.dat"), binary);
      await writeFile(join(repos, "repo.txt"), "repository");
      await writeFile(join(root, "secret"), "host secret");
      await mkdir(join(workspace, "project", "empty"), { recursive: true });
      await mkdir(join(workspace, "project", "src"));
      await mkdir(join(workspace, "project", "cache"));
      await writeFile(join(workspace, "project", "src", "data.bin"), binary);
      await writeFile(join(workspace, "project", "cache", "ignored"), "ignored");
      await writeFile(join(workspace, "project", "bundle.zip"), "old archive");
      const child = spawn(process.execPath, [new URL("../dist/main.js", import.meta.url).pathname], {
        stdio: ["pipe", "pipe", "inherit"],
      });
      const lines = createInterface({ input: child.stdout })[Symbol.asyncIterator]();
      const send = (message) => child.stdin.write(`${JSON.stringify(message)}\n`);
      const receive = async () => {
        const line = await lines.next();
        assert.equal(line.done, false);
        return JSON.parse(line.value);
      };
      try {
        send({ type: "exec", id: 1, sandbox: 1, workspace, repos, team,
          command: `mkdir -p /tmp/output; ln -s /workspace/project /tmp/link; cp binary.dat /tmp/output/copy.dat; cd /tmp/output; ${command} ${command === "discord" ? "send --file copy.dat" : "--output result.zip copy.dat"}; base64 result.zip`,
          timeout_ms: 10000 });
        const call = await receive();
        assert.equal(call.type, "call");
        assert.equal(call.command, command);
        const read = async (path, max_bytes = 1024) => {
          send({ type: "read_file", id: call.id, path, max_bytes });
          const reply = await receive();
          assert.equal(reply.type, "file_result");
          return reply.result;
        };
        if (command === "archive") {
          const operation = async (request) => {
            send({ ...request, id: call.id });
            const reply = await receive();
            assert.equal(reply.type, "file_result");
            return reply.result;
          };
          const walk = { type: "walk_files", paths: ["/workspace/project/"],
            output: "/workspace/project/bundle.zip", recursive: true, exclude: ["cache/**"], max_entries: 256 };
          const walked = await operation(walk);
          assert.equal(walked.error, undefined);
          assert.deepEqual(walked.entries.map((entry) => entry.name), [
            "project", "project/empty", "project/src", "project/src/data.bin",
          ]);
          assert.equal(walked.entries[1].directory, true);
          assert.match((await operation({ ...walk, recursive: false })).error, /recursive/);
          assert.match((await operation({ ...walk, max_entries: 2 })).error, /entries/);
          assert.match((await operation({ ...walk, paths: ["/tmp/link"] })).error, /symlink/);
          const entries = [
            { name: "project/empty", base64: null },
            { name: "project/src/data.bin", base64: binary.toString("base64") },
          ];
          assert.equal(await operation({ type: "write_tree", path: "/tmp/unpacked", entries }), "written");
          assert.deepEqual(Buffer.from((await read("/tmp/unpacked/project/src/data.bin")).base64, "base64"), binary);
          const extracted = await operation({ ...walk, paths: ["/tmp/unpacked"], exclude: [] });
          assert.ok(extracted.entries.some((entry) => entry.name === "unpacked/project/empty" && entry.directory));
          assert.match((await operation({ type: "write_tree", path: "/tmp/unpacked", entries })).error, /already exists/);
          assert.match((await operation({ type: "write_tree", path: "/tmp/link/new", entries })).error, /symlink/);
          for (const name of ["../escape", "/escape", "a/../../escape", "a\\b", "C:escape"]) {
            assert.match((await operation({ type: "write_tree", path: "/tmp/bad", entries: [{ name, base64: "" }] })).error, /unsafe/);
          }
          assert.match((await operation({ type: "write_tree", path: "/tmp/bad", entries: [
            { name: "a", base64: "" }, { name: "a/b", base64: "" },
          ] })).error, /blocks/);
          assert.ok((await read("/tmp/escape")).error);
        }
        const copied = await read("copy.dat");
        assert.equal(copied.error, undefined);
        assert.deepEqual(Buffer.from(copied.base64, "base64"), binary);
        assert.match((await read("copy.dat", 5)).error, /limit/);
        assert.ok((await read("/tmp/output")).error);
        assert.ok((await read(join(root, "secret"))).error);
        assert.ok((await read("/workspace/../secret")).error);
        const repo = await read("/repos/repo.txt");
        if (team) assert.equal(Buffer.from(repo.base64, "base64").toString(), "repository");
        else assert.ok(repo.error);
        send({ type: "reply", id: call.id, result: { command: {
          stdout: "sent\n", stderr: "", exit_code: 0,
          file: { path: "result.zip", base64: binary.toString("base64") },
        } } });
        const completed = await receive();
        assert.equal(completed.type, "exec_result");
        assert.equal(completed.exit_code, 0);
        assert.equal(completed.stdout, `sent\n${binary.toString("base64")}\n`);
        assert.match((await read("copy.dat")).error, /no longer active/);
      } finally {
        child.kill();
        await rm(root, { recursive: true, force: true });
      }
    });
  }
}
