export type BridgeCommand = "discord" | "repo" | "mcp";

export interface ExecRequest {
  type: "exec";
  id: number;
  sandbox: number;
  workspace: string;
  repos: string;
  team: boolean;
  command: string;
  timeout_ms: number;
}

export interface CancelRequest {
  type: "cancel";
  id: number;
}

export interface CloseRequest {
  type: "close";
  sandbox: number;
}

export interface Reply {
  type: "reply";
  id: number;
  result: ReplyResult;
}

export type ReplyResult =
  | { command: CommandOutput }
  | { fetch: FetchResponse }
  | { error: string };

export interface CommandOutput {
  stdout: string;
  stderr: string;
  exit_code: number;
  file: OutputFile | null;
}

export interface OutputFile {
  path: string;
  base64: string;
}

export interface FetchResponse {
  status: number;
  status_text: string;
  headers: Record<string, string>;
  body_base64: string;
  url: string;
}

export type Incoming = ExecRequest | CancelRequest | CloseRequest | Reply;

export interface ExecResult {
  type: "exec_result";
  id: number;
  stdout: string;
  stderr: string;
  exit_code: number;
  images: Image[];
}

export interface Image {
  name: string;
  base64: string;
}

export interface CallRequest {
  type: "call";
  id: number;
  sandbox: number;
  command: BridgeCommand;
  args: string[];
  stdin: string;
}

export interface FetchRequest {
  type: "fetch";
  id: number;
  sandbox: number;
  url: string;
  method: string;
  headers: Record<string, string>;
  body_base64: string | null;
}

export type Outgoing = ExecResult | CallRequest | FetchRequest;
