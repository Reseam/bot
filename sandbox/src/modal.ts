import { readdir, readFile } from "node:fs/promises";
import { posix } from "node:path";
import { fileURLToPath } from "node:url";
import { type App, type Image, ModalClient, NotFoundError, type Sandbox } from "modal";
import { send } from "./bridge.js";
import { errorMessage, failure } from "./output.js";
import type { ExecRequest, ExecResult, ModalBackend, ModalSettings, SaveRequest, Saved } from "./protocol.js";
import { relay } from "./relay.js";

const ROOT = "/opt/reseam-bot";
const WORKSPACE = "/workspace";
const CONTAINER_FILES = fileURLToPath(new URL("../container/", import.meta.url));
const MAX_LIFETIME_MS = 24 * 60 * 60 * 1000;
// Only a crashed bot leaves a sandbox running; a run's own sandbox is saved and stopped when the run ends.
const IDLE_TIMEOUT_MS = 30 * 60 * 1000;
const SNAPSHOT_TTL_MS = 8 * 60 * 60 * 1000;
const EXEC_GRACE_MS = 60 * 1000;

interface Modal {
  client: ModalClient;
  settings: ModalSettings;
  app: Promise<App>;
}

interface Session {
  sandbox: Sandbox;
  tail: Promise<void>;
  notice: string;
}

let modal: Modal | null = null;
const sessions = new Map<number, Promise<Session>>();
const executions = new Map<number, AbortController>();

export function configure(settings: ModalSettings | null): void {
  if (settings === null) {
    modal = null;
    return;
  }
  const client = new ModalClient({ tokenId: settings.token_id, tokenSecret: settings.token_secret });
  modal = { client, settings, app: client.apps.fromName(settings.app, { createIfMissing: true }) };
}

export async function execute(message: ExecRequest<ModalBackend>): Promise<void> {
  const controller = new AbortController();
  executions.set(message.id, controller);
  try {
    const session = await open(message.sandbox, message.backend.image);
    const turn = session.tail.then(() => run(session, message, controller.signal));
    session.tail = turn.then(() => undefined);
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

export async function save(message: SaveRequest): Promise<void> {
  const pending = sessions.get(message.sandbox);
  sessions.delete(message.sandbox);
  const session = await pending?.catch(() => null);
  if (!session) {
    send({ type: "saved", id: message.id, result: "unchanged" });
    return;
  }
  let result: Saved["result"];
  try {
    await session.tail;
    const image = await session.sandbox.snapshotFilesystem({ ttlMs: SNAPSHOT_TTL_MS });
    result = { image: image.imageId };
  } catch (error) {
    result = { error: errorMessage(error) };
  }
  await session.sandbox.terminate().catch((error: unknown) => {
    console.error(`failed to stop sandbox ${session.sandbox.sandboxId}: ${errorMessage(error)}`);
  });
  if ("image" in result && message.previous !== null) {
    await configured()
      .client.images.delete(message.previous)
      .catch((error: unknown) => {
        if (!(error instanceof NotFoundError)) {
          console.error(`failed to delete sandbox image ${message.previous}: ${errorMessage(error)}`);
        }
      });
  }
  send({ type: "saved", id: message.id, result });
}

async function run(session: Session, message: ExecRequest<ModalBackend>, signal: AbortSignal): Promise<ExecResult> {
  const process = await session.sandbox.exec(["python3", `${ROOT}/relay.py`], {
    timeoutMs: message.timeout_ms + EXEC_GRACE_MS,
  });
  const crash = process.stderr.readText().catch(() => "");
  try {
    const result = await relay(
      process,
      message.sandbox,
      { command: message.command, cwd: WORKSPACE, path: `${ROOT}/bin`, timeout_ms: message.timeout_ms },
      signal,
    );
    const notice = session.notice;
    session.notice = "";
    return { type: "exec_result", id: message.id, ...result, stderr: notice + result.stderr };
  } catch (error) {
    throw new Error(`${errorMessage(error)}\n${await crash}`.trimEnd());
  }
}

function configured(): Modal {
  if (modal === null) {
    throw new Error("the real sandbox is not configured");
  }
  return modal;
}

function open(conversation: number, image: string | null): Promise<Session> {
  const existing = sessions.get(conversation);
  if (existing) {
    return existing;
  }
  const created = create(conversation, image);
  sessions.set(conversation, created);
  created.catch(() => sessions.delete(conversation));
  return created;
}

async function create(conversation: number, image: string | null): Promise<Session> {
  const { client, settings, app } = configured();
  let source: Image;
  let notice = "";
  if (image === null) {
    source = await client.images.fromName(settings.image);
  } else {
    try {
      source = await client.images.fromId(image);
    } catch (error) {
      if (!(error instanceof NotFoundError)) {
        throw error;
      }
      source = await client.images.fromName(settings.image);
      notice = "Earlier container files expired, so this run started from a fresh container.\n";
    }
  }
  const sandbox = await client.sandboxes.create(await app, source, {
    cpu: settings.cpu,
    memoryMiB: settings.memory_mib,
    timeoutMs: MAX_LIFETIME_MS,
    idleTimeoutMs: IDLE_TIMEOUT_MS,
    workdir: WORKSPACE,
    tags: { conversation: String(conversation) },
  });
  try {
    return { sandbox, tail: Promise.resolve(), notice: notice + (await install(sandbox)) };
  } catch (error) {
    await sandbox.terminate().catch(() => undefined);
    throw error;
  }
}

async function install(sandbox: Sandbox): Promise<string> {
  const writes = (await containerFiles()).map((file) => {
    const path = posix.join(ROOT, file.path);
    return `mkdir -p ${posix.dirname(path)}\nprintf %s ${file.bytes.toString("base64")} | base64 -d > ${path}`;
  });
  const script = [
    "set -eo pipefail",
    `mkdir -p ${WORKSPACE}`,
    ...writes,
    `chmod +x ${ROOT}/bin/*`,
    `${ROOT}/bin/reseam-update || echo "reseam-update failed, so the Reseam tools may be outdated." >&2`,
  ].join("\n");
  const setup = await sandbox.exec(["bash", "-s"], { stdout: "ignore" });
  const writer = setup.stdin.getWriter();
  await writer.write(`${script}\n`);
  await writer.close();
  const [stderr, code] = await Promise.all([setup.stderr.readText(), setup.wait()]);
  if (code !== 0) {
    throw new Error(`sandbox setup failed: ${stderr}`);
  }
  return stderr;
}

let files: Promise<{ path: string; bytes: Buffer }[]> | null = null;

function containerFiles(): Promise<{ path: string; bytes: Buffer }[]> {
  files ??= readdir(CONTAINER_FILES, { recursive: true, withFileTypes: true }).then((entries) =>
    Promise.all(
      entries
        .filter((entry) => entry.isFile())
        .map(async (entry) => {
          const absolute = posix.join(entry.parentPath, entry.name);
          return {
            path: posix.relative(CONTAINER_FILES, absolute),
            bytes: await readFile(absolute),
          };
        }),
    ),
  );
  return files;
}
