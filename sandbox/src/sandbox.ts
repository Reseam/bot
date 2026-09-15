import {
  Bash,
  type Command,
  decodeBytesToUtf8,
  defineCommand,
  InMemoryFs,
  MountableFs,
  OverlayFs,
  ReadWriteFs,
  type SecureFetch,
  stdoutAsBytes,
  stdoutKind,
} from "just-bash";
import { request, send } from "./bridge.js";
import type { BridgeCommand, ExecRequest, ExecResult, Image } from "./protocol.js";

const BRIDGE_COMMANDS: BridgeCommand[] = ["discord", "repo", "mcp"];
const REPO_OVERLAY_BYTES = 256 * 1024 * 1024;
const MAX_EXECUTION_MS = 30 * 60 * 1000;
const PYTHON_TIMEOUT_MS = 10 * 60 * 1000;

interface Sandbox {
  bash: Bash;
  images: Image[];
  tail: Promise<void>;
}

const sandboxes = new Map<number, Promise<Sandbox>>();
const executions = new Map<number, AbortController>();

export async function execute(message: ExecRequest): Promise<void> {
  const controller = new AbortController();
  executions.set(message.id, controller);
  try {
    const sandbox = await open(message);
    const turn = sandbox.tail.then(() => run(sandbox, message, controller.signal));
    sandbox.tail = turn.then(() => undefined);
    send(await turn);
  } catch (error) {
    send(failure(message.id, error, []));
  } finally {
    executions.delete(message.id);
  }
}

export function cancel(id: number): void {
  executions.get(id)?.abort(new Error("cancelled"));
}

export function close(sandbox: number): void {
  sandboxes.delete(sandbox);
}

async function run(sandbox: Sandbox, message: ExecRequest, cancelled: AbortSignal): Promise<ExecResult> {
  sandbox.images.length = 0;
  const timeout = AbortSignal.timeout(message.timeout_ms);
  const signal = AbortSignal.any([cancelled, timeout]);
  try {
    const result = await sandbox.bash.exec(message.command, { signal });
    const stderr = timeout.aborted
      ? `${result.stderr}timed out after ${message.timeout_ms / 1000}s\n`
      : result.stderr;
    return {
      type: "exec_result",
      id: message.id,
      stdout: stdoutKind(result) === "bytes" ? decodeBytesToUtf8(stdoutAsBytes(result)) : result.stdout,
      stderr,
      exit_code: result.exitCode,
      images: sandbox.images.splice(0),
    };
  } catch (error) {
    return failure(message.id, error, sandbox.images.splice(0));
  }
}

function failure(id: number, error: unknown, images: Image[]): ExecResult {
  return { type: "exec_result", id, stdout: "", stderr: `${errorMessage(error)}\n`, exit_code: 1, images };
}

function open(message: ExecRequest): Promise<Sandbox> {
  const existing = sandboxes.get(message.sandbox);
  if (existing) {
    return existing;
  }
  const created = create(message);
  sandboxes.set(message.sandbox, created);
  created.catch(() => sandboxes.delete(message.sandbox));
  return created;
}

async function create(message: ExecRequest): Promise<Sandbox> {
  const base = new InMemoryFs();
  await base.mkdir("/tmp", { recursive: true });
  const fs = new MountableFs({ base });
  fs.mount("/workspace", new ReadWriteFs({ root: message.workspace }));
  fs.mount(
    "/repos",
    new OverlayFs({ root: message.repos, mountPoint: "/", maxMemoryBytes: REPO_OVERLAY_BYTES }),
  );
  const images: Image[] = [];
  const bash = new Bash({
    fs,
    cwd: "/workspace",
    env: { HOME: "/workspace" },
    python: true,
    javascript: true,
    fetch: bridgeFetch(message.sandbox),
    customCommands: [
      ...BRIDGE_COMMANDS.map((name) => bridgeCommand(message.sandbox, name)),
      viewCommand(images),
    ],
    executionLimits: { maxExecutionTimeMs: MAX_EXECUTION_MS, maxPythonTimeoutMs: PYTHON_TIMEOUT_MS },
  });
  return { bash, images, tail: Promise.resolve() };
}

function bridgeCommand(sandbox: number, name: BridgeCommand): Command {
  return defineCommand(name, async (args, ctx) => {
    const result = await request(
      { type: "call", sandbox, command: name, args, stdin: decodeBytesToUtf8(ctx.stdin) },
      ctx.signal,
    );
    if ("error" in result) {
      return { stdout: "", stderr: `${name}: ${result.error}\n`, exitCode: 1 };
    }
    if (!("command" in result)) {
      throw new Error(`unexpected reply to ${name}`);
    }
    const output = result.command;
    if (output.file) {
      await ctx.fs.writeFile(
        ctx.fs.resolvePath(ctx.cwd, output.file.path),
        Buffer.from(output.file.base64, "base64"),
      );
    }
    return { stdout: output.stdout, stderr: output.stderr, exitCode: output.exit_code };
  });
}

function viewCommand(images: Image[]): Command {
  return defineCommand("view", async (args, ctx) => {
    if (args.length === 0) {
      return { stdout: "", stderr: "usage: view FILE...\n", exitCode: 2 };
    }
    for (const path of args) {
      try {
        const bytes = await ctx.fs.readFileBuffer(ctx.fs.resolvePath(ctx.cwd, path));
        images.push({ name: path, base64: Buffer.from(bytes).toString("base64") });
      } catch (error) {
        return { stdout: "", stderr: `view: ${path}: ${errorMessage(error)}\n`, exitCode: 1 };
      }
    }
    return { stdout: `Attached ${args.join(", ")} to this result.\n`, stderr: "", exitCode: 0 };
  });
}

function bridgeFetch(sandbox: number): SecureFetch {
  return async (url, options = {}) => {
    const result = await request(
      {
        type: "fetch",
        sandbox,
        url,
        method: options.method?.toUpperCase() ?? "GET",
        headers: Object.fromEntries(new Headers(options.headers)),
        body: options.body ?? null,
      },
      options.signal,
    );
    if ("error" in result) {
      throw new Error(result.error);
    }
    if (!("fetch" in result)) {
      throw new Error("unexpected reply to fetch");
    }
    const response = result.fetch;
    return {
      status: response.status,
      statusText: response.status_text,
      headers: response.headers,
      body: Buffer.from(response.body_base64, "base64"),
      url: response.url,
    };
  };
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
