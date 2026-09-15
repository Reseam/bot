import type { CallRequest, FetchRequest, Outgoing, ReplyResult } from "./protocol.js";

export type BridgeRequest = Omit<CallRequest, "id"> | Omit<FetchRequest, "id">;

const waiting = new Map<number, (result: ReplyResult) => void>();
let nextId = 1;

export function send(message: Outgoing): void {
  process.stdout.write(`${JSON.stringify(message)}\n`);
}

export function request(message: BridgeRequest, signal: AbortSignal | undefined): Promise<ReplyResult> {
  const id = nextId++;
  return new Promise((resolve, reject) => {
    const abort = () => {
      waiting.delete(id);
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
    send({ ...message, id } as Outgoing);
  });
}

export function resolveReply(id: number, result: ReplyResult): void {
  const resolve = waiting.get(id);
  waiting.delete(id);
  resolve?.(result);
}
