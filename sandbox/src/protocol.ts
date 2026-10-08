export type BridgeCommand = "discord" | "repo" | "mcp" | "archive";

export interface ModalSettings {
  token_id: string;
  token_secret: string;
  app: string;
  image: string;
  cpu: number;
  memory_mib: number;
}

export interface ConfigureRequest {
  type: "configure";
  modal: ModalSettings | null;
}

export interface JustBashBackend {
  kind: "just-bash";
  workspace: string;
  repos: string;
  team: boolean;
}

export interface ModalBackend {
  kind: "modal";
  image: string | null;
}

export interface ExecRequest<Backend = JustBashBackend | ModalBackend> {
  type: "exec";
  id: number;
  sandbox: number;
  command: string;
  timeout_ms: number;
  backend: Backend;
}

export interface SaveRequest {
  type: "save";
  id: number;
  sandbox: number;
  previous: string | null;
}

export interface Saved {
  type: "saved";
  id: number;
  result: { image: string } | "unchanged" | { error: string };
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

export interface ReadFileRequest {
  type: "read_file";
  id: number;
  path: string;
  max_bytes: number;
}

export interface WalkFilesRequest {
  type: "walk_files";
  id: number;
  paths: string[];
  output: string;
  recursive: boolean;
  exclude: string[];
  max_entries: number;
}

export interface ArchiveEntry {
  path: string;
  name: string;
  directory: boolean;
}

export interface WriteTreeRequest {
  type: "write_tree";
  id: number;
  path: string;
  entries: { name: string; base64: string | null }[];
}

export type FileRequest = ReadFileRequest | WalkFilesRequest | WriteTreeRequest;

export interface FileResult {
  type: "file_result";
  id: number;
  result: { base64: string } | { error: string } | { entries: ArchiveEntry[] } | "written";
}

export type Incoming = ConfigureRequest | ExecRequest | SaveRequest | CancelRequest | CloseRequest | Reply | FileRequest;

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

export type Outgoing = ExecResult | Saved | CallRequest | FetchRequest | FileResult;
