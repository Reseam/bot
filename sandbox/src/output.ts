import type { ExecResult, Image } from "./protocol.js";

const BINARY_MIN_SUSPICIOUS_CHARS = 8;

export function textOrOmitted(text: string, bytes?: number): string {
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

export function failure(id: number, error: unknown, images: Image[]): ExecResult {
  return { type: "exec_result", id, stdout: "", stderr: `${errorMessage(error)}\n`, exit_code: 1, images };
}

export function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
