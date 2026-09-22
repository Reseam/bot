import type { CommandContext } from "just-bash";
import { posix } from "node:path";
import type { ArchiveEntry, FileRequest, FileResult, WalkFilesRequest, WriteTreeRequest } from "./protocol.js";

export async function fileOperation(ctx: CommandContext, request: FileRequest): Promise<FileResult["result"]> {
  ctx.signal?.throwIfAborted();
  switch (request.type) {
    case "walk_files":
      return { entries: await walk(ctx, request) };
    case "write_tree":
      await writeTree(ctx, request);
      return "written";
    case "read_file": {
      const path = ctx.fs.resolvePath(ctx.cwd, request.path);
      const stat = await ctx.fs.stat(path);
      if (!stat.isFile) throw new Error("not a regular file");
      if (stat.size > request.max_bytes) throw new Error(`file exceeds the ${request.max_bytes} byte limit`);
      const bytes = await ctx.fs.readFileBuffer(path);
      if (bytes.length > request.max_bytes) throw new Error(`file exceeds the ${request.max_bytes} byte limit`);
      ctx.signal?.throwIfAborted();
      return { base64: Buffer.from(bytes).toString("base64") };
    }
  }
}

async function noSymlinks(ctx: CommandContext, path: string): Promise<void> {
  for (let current = path; current !== "/"; current = posix.dirname(current)) {
    if ((await ctx.fs.lstat(current)).isSymbolicLink) {
      throw new Error(`symlinks are not supported: ${current}`);
    }
  }
}

function safeName(name: string): void {
  if (name.split("/").some((part) => !part || part === "." || part === "..") || /[\\:\x00-\x1f\x7f]/u.test(name)) {
    throw new Error(`unsafe archive path: ${name}`);
  }
}

async function walk(ctx: CommandContext, request: WalkFilesRequest): Promise<ArchiveEntry[]> {
  const entries: ArchiveEntry[] = [];
  const seen = new Set<string>();
  const output = ctx.fs.resolvePath(ctx.cwd, request.output);
  async function visit(path: string, name: string, relative: string): Promise<void> {
    ctx.signal?.throwIfAborted();
    if (path === output) return;
    if (request.exclude.some((pattern) => [name, relative, posix.basename(path), `${relative}/`]
      .some((candidate) => posix.matchesGlob(candidate, pattern)))) return;
    safeName(name);
    const stat = await ctx.fs.lstat(path);
    if (stat.isSymbolicLink) throw new Error(`symlinks are not supported: ${path}`);
    if (!stat.isFile && !stat.isDirectory) throw new Error(`not a regular file or directory: ${path}`);
    if (stat.isDirectory && !request.recursive) throw new Error(`use --recursive to include directory ${path}`);
    if (seen.has(name)) throw new Error(`duplicate archive path: ${name}`);
    if (entries.length >= request.max_entries) throw new Error(`archive exceeds ${request.max_entries} entries`);
    seen.add(name);
    entries.push({ path, name, directory: stat.isDirectory });
    if (stat.isDirectory) {
      for (const child of (await ctx.fs.readdir(path)).sort()) {
        await visit(posix.join(path, child), `${name}/${child}`, relative ? `${relative}/${child}` : child);
      }
    }
  }
  for (const input of request.paths) {
    const path = ctx.fs.resolvePath(ctx.cwd, input);
    await noSymlinks(ctx, path);
    await visit(path, posix.basename(path), "");
  }
  if (!entries.length) throw new Error("no files or directories selected");
  return entries;
}

async function writeTree(ctx: CommandContext, request: WriteTreeRequest): Promise<void> {
  const root = ctx.fs.resolvePath(ctx.cwd, request.path);
  const parent = posix.dirname(root);
  await noSymlinks(ctx, parent);
  if ((await ctx.fs.readdir(parent)).includes(posix.basename(root)) || root === "/") {
    throw new Error("extraction destination already exists; choose a new directory");
  }
  const names = new Map<string, boolean>();
  for (const entry of request.entries) {
    safeName(entry.name);
    if (names.has(entry.name)) throw new Error(`duplicate archive path: ${entry.name}`);
    names.set(entry.name, entry.base64 === null);
  }
  for (const entry of request.entries) {
    for (let parent = posix.dirname(entry.name); parent !== "."; parent = posix.dirname(parent)) {
      if (names.get(parent) === false) throw new Error(`file blocks archive directory: ${parent}`);
    }
  }
  await ctx.fs.mkdir(root);
  try {
    for (const entry of request.entries) {
      ctx.signal?.throwIfAborted();
      const path = posix.join(root, entry.name);
      if (entry.base64 === null) {
        await ctx.fs.mkdir(path, { recursive: true });
      } else {
        await ctx.fs.mkdir(posix.dirname(path), { recursive: true });
        await ctx.fs.writeFile(path, Buffer.from(entry.base64, "base64"));
      }
    }
  } catch (error) {
    try {
      await ctx.fs.rm(root, { recursive: true });
    } catch (cleanup) {
      throw new AggregateError([error, cleanup], "extraction failed and cleanup failed");
    }
    throw error;
  }
}
