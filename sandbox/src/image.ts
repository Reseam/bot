import { readFile } from "node:fs/promises";
import { ModalClient } from "modal";

const [appName, imageName] = process.argv.slice(2);
if (appName === undefined || imageName === undefined) {
  console.error("usage: node dist/image.js APP IMAGE");
  process.exit(2);
}
const [from = "", ...commands] = (await readFile(new URL("../image/Dockerfile", import.meta.url), "utf8"))
  .trimEnd()
  .split("\n");
const base = /^FROM (\S+)$/.exec(from)?.[1];
if (base === undefined) {
  throw new Error("image/Dockerfile must start with a FROM line");
}
const modal = new ModalClient();
const app = await modal.apps.fromName(appName, { createIfMissing: true });
const image = await modal.images.fromRegistry(base).dockerfileCommands(commands).build(app);
await image.publish(imageName);
console.log(`Published ${imageName} as ${image.imageId}`);
modal.close();
