// Pocket New York's commands.
//
//   bun tools/newyork.ts three | serve | look | export | view

import { exportScene } from "./export";
import { look } from "./look";
import { view } from "./view";
import { fetchThree, serve } from "./web";

const [command, ...argv] = process.argv.slice(2);
switch (command) {
  case "three":
    await fetchThree();
    break;
  case "serve":
    serve();
    break;
  case "export":
    await exportScene(argv);
    break;
  case "view":
    await view(argv);
    break;
  case "look":
    await look(argv);
    break;
  default:
    console.error("usage: bun tools/newyork.ts three | serve | look | export | view");
    process.exit(1);
}
