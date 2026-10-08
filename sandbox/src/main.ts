import { createInterface } from "node:readline";
import { readFile, resolveReply } from "./bridge.js";
import * as justBash from "./justbash.js";
import * as modal from "./modal.js";
import type { Incoming } from "./protocol.js";

for await (const line of createInterface({ input: process.stdin, crlfDelay: Infinity })) {
  const message = JSON.parse(line) as Incoming;
  switch (message.type) {
    case "configure":
      modal.configure(message.modal);
      break;
    case "exec": {
      const { backend, ...request } = message;
      if (backend.kind === "modal") {
        void modal.execute({ ...request, backend });
      } else {
        void justBash.execute({ ...request, backend });
      }
      break;
    }
    case "save":
      void modal.save(message);
      break;
    case "cancel":
      justBash.cancel(message.id);
      modal.cancel(message.id);
      break;
    case "close":
      justBash.close(message.sandbox);
      break;
    case "walk_files":
    case "write_tree":
    case "read_file":
      void readFile(message);
      break;
    case "reply":
      resolveReply(message.id, message.result);
      break;
  }
}
process.exit(0);
