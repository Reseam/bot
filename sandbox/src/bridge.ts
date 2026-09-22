import { AsyncResource } from "node:async_hooks";
import type { CallRequest, FetchRequest, Outgoing, ReplyResult, FileRequest, FileResult } from "./protocol.js";

export type BridgeRequest = Omit<CallRequest, "id"> | Omit<FetchRequest, "id">;

const waiting = new Map<number, (result: ReplyResult) => void>();
const fileReaders = new Map<number, (request: FileRequest) => Promise<FileResult["result"]>>();
let nextId = 1;

export function send(message: Outgoing): void {
  process.stdout.write(`${JSON.stringify(message)}\n`);
}

export function request(
  message: BridgeRequest,
  signal: AbortSignal | undefined,
  readFile?: (request: FileRequest) => Promise<FileResult["result"]>,
): Promise<ReplyResult> {
  const id = nextId++;
  return new Promise((resolve, reject) => {
    const abort = () => {
      waiting.delete(id);
      fileReaders.delete(id);
      reject(signal?.reason);
    };
    if (signal?.aborted) {
      abort();
      return;
    }
    signal?.addEventListener("abort", abort, { once: true });
    waiting.set(id, (result) => {
      signal?.removeEventListener("abort", abort);
      resolve(result);
    });
    // File requests arrive on stdin; retain the command's filesystem security context.
    if (readFile) {
      fileReaders.set(id, AsyncResource.bind(readFile));
    }
    send({ ...message, id } as Outgoing);
  });
}

export function resolveReply(id: number, result: ReplyResult): void {
  const resolve = waiting.get(id);
  waiting.delete(id);
  fileReaders.delete(id);
  resolve?.(result);
}

export async function readFile(message: FileRequest): Promise<void> {
  try {
    const reader = fileReaders.get(message.id);
    if (!reader) {
      throw new Error("command is no longer active");
    }
    const result = await reader(message);
    send({ type: "file_result", id: message.id, result });
  } catch (error) {
    send({
      type: "file_result",
      id: message.id,
      result: { error: error instanceof Error ? error.message : String(error) },
    });
  }
}
