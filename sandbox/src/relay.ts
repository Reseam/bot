import { request } from "./bridge.js";
import { errorMessage, textOrOmitted } from "./output.js";
import type { BridgeCommand, ExecResult, FileRequest, FileResult, Image, ReplyResult } from "./protocol.js";

export interface RelayStart {
  command: string;
  cwd: string;
  path: string;
  timeout_ms: number;
}

export interface RelayProcess {
  stdin: WritableStream<string>;
  stdout: ReadableStream<string>;
}

type CommandOutput = Omit<ExecResult, "type" | "id">;

type BridgeMessage =
  | { type: "call"; id: number; command: BridgeCommand; args: string[]; stdin: string }
  | {
      type: "fetch";
      id: number;
      url: string;
      method: string;
      headers: Record<string, string>;
      body_base64: string | null;
    };

interface RelayResult {
  type: "exec_result";
  stdout: string;
  stdout_size: number;
  stderr: string;
  stderr_size: number;
  exit_code: number;
  timed_out: boolean;
  cancelled: boolean;
  images: Image[];
}

type RelayMessage = BridgeMessage | { type: "file_result"; id: number; result: FileResult["result"] } | RelayResult;

export async function relay(
  process: RelayProcess,
  sandbox: number,
  start: RelayStart,
  signal: AbortSignal,
): Promise<CommandOutput> {
  const writer = process.stdin.getWriter();
  const write = (message: object) => writer.write(`${JSON.stringify(message)}\n`).catch(() => undefined);
  const files = new Map<number, (result: FileResult["result"]) => void>();
  let nextFile = 1;

  const readFile =
    (call: number) =>
    (file: FileRequest): Promise<FileResult["result"]> => {
      if (file.type !== "read_file") {
        return Promise.resolve({ error: "not available in this sandbox" });
      }
      return new Promise((resolve) => {
        const id = nextFile++;
        files.set(id, resolve);
        void write({ type: "read_file", id, call, path: file.path, max_bytes: file.max_bytes });
      });
    };

  const forward = async ({ id, ...message }: BridgeMessage) => {
    let result: ReplyResult;
    try {
      result = await request(
        { ...message, sandbox },
        signal,
        message.type === "call" ? readFile(id) : undefined,
      );
    } catch (error) {
      result = { error: errorMessage(error) };
    }
    await write({ type: "reply", id, result });
  };

  const cancel = () => void writer.close().catch(() => undefined);
  signal.addEventListener("abort", cancel, { once: true });
  try {
    await write(start);
    for await (const line of lines(process.stdout)) {
      const message = JSON.parse(line) as RelayMessage;
      switch (message.type) {
        case "call":
        case "fetch":
          void forward(message);
          break;
        case "file_result":
          files.get(message.id)?.(message.result);
          files.delete(message.id);
          break;
        case "exec_result":
          return output(message, start.timeout_ms);
      }
    }
  } finally {
    signal.removeEventListener("abort", cancel);
  }
  throw new Error("the sandbox command ended without a result");
}

async function* lines(stream: ReadableStream<string>): AsyncGenerator<string> {
  let buffered = "";
  for await (const chunk of stream) {
    buffered += chunk;
    let newline = buffered.indexOf("\n");
    while (newline !== -1) {
      yield buffered.slice(0, newline);
      buffered = buffered.slice(newline + 1);
      newline = buffered.indexOf("\n");
    }
  }
}

function output(result: RelayResult, timeoutMs: number): CommandOutput {
  let stderr = decode(result.stderr, result.stderr_size);
  if (result.timed_out) {
    stderr += `timed out after ${timeoutMs / 1000}s\n`;
  }
  if (result.cancelled) {
    stderr += "cancelled\n";
  }
  return {
    stdout: decode(result.stdout, result.stdout_size),
    stderr,
    exit_code: result.exit_code,
    images: result.images,
  };
}

function decode(base64: string, size: number): string {
  const bytes = Buffer.from(base64, "base64");
  const text = textOrOmitted(new TextDecoder().decode(bytes), size);
  return size > bytes.length ? `${text}\n[output truncated: ${size} bytes in total]\n` : text;
}
