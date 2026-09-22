import { fileOperation } from "./files.js";
import {
  Bash,
  type ByteString,
  type Command,
  decodeBytesToUtf8,
  defineCommand,
  InMemoryFs,
  latin1FromBytes,
  MountableFs,
  OverlayFs,
  ReadWriteFs,
  type SecureFetch,
  stdoutAsBytes,
  stdoutKind,
} from "just-bash";
import { randomUUID } from "node:crypto";
import { posix } from "node:path";
import { request, send } from "./bridge.js";
import type { BridgeCommand, ExecRequest, ExecResult, FetchResponse, Image } from "./protocol.js";

const TEAM_BRIDGE_COMMANDS: BridgeCommand[] = ["discord", "repo", "mcp", "archive"];
const REPO_OVERLAY_BYTES = 256 * 1024 * 1024;
const BINARY_MIN_SUSPICIOUS_CHARS = 8;
const MAX_EXECUTION_MS = 30 * 60 * 1000;
const PYTHON_TIMEOUT_MS = 10 * 60 * 1000;

interface Sandbox {
  team: Bash;
  member: Bash;
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
    const bash = message.team ? sandbox.team : sandbox.member;
    const result = await bash.exec(message.command, { signal });
    const stderr = textOrOmitted(result.stderr);
    return {
      type: "exec_result",
      id: message.id,
      stdout:
        stdoutKind(result) === "bytes" ? bytesOrOmitted(stdoutAsBytes(result)) : textOrOmitted(result.stdout),
      stderr: timeout.aborted ? `${stderr}timed out after ${message.timeout_ms / 1000}s\n` : stderr,
      exit_code: result.exitCode,
      images: sandbox.images.splice(0),
    };
  } catch (error) {
    return failure(message.id, error, sandbox.images.splice(0));
  }
}

function bytesOrOmitted(bytes: ByteString): string {
  return textOrOmitted(decodeBytesToUtf8(bytes), latin1FromBytes(bytes).length);
}

function textOrOmitted(text: string, bytes?: number): string {
  let suspicious = 0;
  for (const char of text) {
    if (char === "\uFFFD" || (char < " " && char !== "\n" && char !== "\r" && char !== "\t" && char !== "\x1b")) {
      suspicious++;
    }
  }
  return suspicious >= BINARY_MIN_SUSPICIOUS_CHARS && suspicious * 10 > text.length ? omitted(bytes) : text;
}

function omitted(bytes: number | undefined): string {
  const size = bytes === undefined ? "" : `: ${bytes} bytes`;
  return `[binary output omitted${size}. Write binary data to a file with -o or > FILE instead of printing it.]\n`;
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
  const workspace = new ReadWriteFs({ root: message.workspace });
  const teamFs = await filesystem(workspace);
  teamFs.mount(
    "/repos",
    new OverlayFs({ root: message.repos, mountPoint: "/", maxMemoryBytes: REPO_OVERLAY_BYTES }),
  );
  const images: Image[] = [];
  const shared = {
    cwd: "/workspace",
    env: { HOME: "/workspace" },
    executionLimits: { maxExecutionTimeMs: MAX_EXECUTION_MS, maxPythonTimeoutMs: PYTHON_TIMEOUT_MS },
  };
  const team = new Bash({
    ...shared,
    fs: teamFs,
    python: true,
    javascript: true,
    fetch: bridgeFetch(message.sandbox),
    customCommands: [
      ...TEAM_BRIDGE_COMMANDS.map((name) => bridgeCommand(message.sandbox, name)),
      uploadCommand(message.sandbox),
      viewCommand(images),
    ],
  });
  const member = new Bash({
    ...shared,
    fs: await filesystem(workspace),
    customCommands: [
      bridgeCommand(message.sandbox, "discord"),
      bridgeCommand(message.sandbox, "archive"),
      viewCommand(images),
    ],
  });
  return { team, member, images, tail: Promise.resolve() };
}

async function filesystem(workspace: ReadWriteFs): Promise<MountableFs> {
  const base = new InMemoryFs();
  await base.mkdir("/tmp", { recursive: true });
  const fs = new MountableFs({ base });
  fs.mount("/workspace", workspace);
  return fs;
}

function bridgeCommand(sandbox: number, name: BridgeCommand): Command {
  return defineCommand(name, async (args, ctx) => {
    const result = await request(
      { type: "call", sandbox, command: name, args, stdin: decodeBytesToUtf8(ctx.stdin) },
      ctx.signal,
      (request) => fileOperation(ctx, request),
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

const UPLOAD_USAGE = `usage: upload [--method METHOD] [--header 'Name: value']... [--form FIELD] URL FILE
Send FILE's exact bytes: as a multipart/form-data part named FIELD with --form, otherwise as the raw request body.
Prints the response body and exits 22 on an HTTP error status.`;

interface Upload {
  method: string;
  headers: Record<string, string>;
  form: string | null;
  url: string;
  file: string;
}

function parseUpload(args: string[]): Upload | string {
  const rest = [...args];
  const positional: string[] = [];
  let method = "POST";
  let form: string | null = null;
  const headers: Record<string, string> = {};
  for (let arg = rest.shift(); arg !== undefined; arg = rest.shift()) {
    if (arg === "--method" || arg === "-X") {
      method = (rest.shift() ?? "").toUpperCase();
    } else if (arg === "--header" || arg === "-H") {
      const header = rest.shift() ?? "";
      const colon = header.indexOf(":");
      if (colon < 1) {
        return `invalid header: ${header}`;
      }
      headers[header.slice(0, colon).trim().toLowerCase()] = header.slice(colon + 1).trim();
    } else if (arg === "--form" || arg === "-F") {
      form = rest.shift() ?? "";
    } else if (arg.startsWith("-")) {
      return `unknown option: ${arg}`;
    } else {
      positional.push(arg);
    }
  }
  const [url, file, ...extra] = positional;
  if (url === undefined || file === undefined || extra.length > 0) {
    return "expected URL and FILE";
  }
  return { method, headers, form, url, file };
}

function uploadCommand(sandbox: number): Command {
  return defineCommand("upload", async (args, ctx) => {
    if (args.includes("--help") || args.includes("-h")) {
      return { stdout: `${UPLOAD_USAGE}\n`, stderr: "", exitCode: 0 };
    }
    const upload = parseUpload(args);
    if (typeof upload === "string") {
      return { stdout: "", stderr: `upload: ${upload}\n${UPLOAD_USAGE}\n`, exitCode: 2 };
    }
    let bytes: Buffer;
    try {
      bytes = Buffer.from(await ctx.fs.readFileBuffer(ctx.fs.resolvePath(ctx.cwd, upload.file)));
    } catch (error) {
      return { stdout: "", stderr: `upload: ${upload.file}: ${errorMessage(error)}\n`, exitCode: 1 };
    }
    let body = bytes;
    if (upload.form !== null) {
      const boundary = `reseam-${randomUUID()}`;
      const filename = posix.basename(upload.file).replaceAll('"', "");
      body = Buffer.concat([
        Buffer.from(
          `--${boundary}\r\nContent-Disposition: form-data; name="${upload.form}"; filename="${filename}"\r\n` +
            "Content-Type: application/octet-stream\r\n\r\n",
        ),
        bytes,
        Buffer.from(`\r\n--${boundary}--\r\n`),
      ]);
      upload.headers["content-type"] = `multipart/form-data; boundary=${boundary}`;
    } else {
      upload.headers["content-type"] ??= "application/octet-stream";
    }
    try {
      const response = await fetchBytes(sandbox, upload.url, upload.method, upload.headers, body, ctx.signal);
      const text = Buffer.from(response.body_base64, "base64").toString("utf8");
      if (response.status >= 400) {
        return { stdout: text, stderr: `upload: HTTP ${response.status} ${response.status_text}\n`, exitCode: 22 };
      }
      return { stdout: text, stderr: "", exitCode: 0 };
    } catch (error) {
      return { stdout: "", stderr: `upload: ${errorMessage(error)}\n`, exitCode: 1 };
    }
  });
}

async function fetchBytes(
  sandbox: number,
  url: string,
  method: string,
  headers: Record<string, string>,
  body: Buffer | null,
  signal: AbortSignal | undefined,
): Promise<FetchResponse> {
  const result = await request(
    { type: "fetch", sandbox, url, method, headers, body_base64: body?.toString("base64") ?? null },
    signal,
  );
  if ("error" in result) {
    throw new Error(result.error);
  }
  if (!("fetch" in result)) {
    throw new Error("unexpected reply to fetch");
  }
  return result.fetch;
}

function bridgeFetch(sandbox: number): SecureFetch {
  return async (url, options = {}) => {
    const response = await fetchBytes(
      sandbox,
      url,
      options.method?.toUpperCase() ?? "GET",
      Object.fromEntries(new Headers(options.headers)),
      options.body === undefined ? null : Buffer.from(options.body, "utf8"),
      options.signal,
    );
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
