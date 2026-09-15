import { createInterface } from "node:readline";
import { resolveReply } from "./bridge.js";
import type { Incoming } from "./protocol.js";
import { cancel, close, execute } from "./sandbox.js";

for await (const line of createInterface({ input: process.stdin, crlfDelay: Infinity })) {
  const message = JSON.parse(line) as Incoming;
  switch (message.type) {
    case "exec":
      void execute(message);
      break;
    case "cancel":
      cancel(message.id);
      break;
    case "close":
      close(message.sandbox);
      break;
    case "reply":
      resolveReply(message.id, message.result);
      break;
  }
}
process.exit(0);
