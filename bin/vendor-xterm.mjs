import { copyFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");

const files = [
  ["@xterm/xterm/lib/xterm.js", "xterm.js"],
  ["@xterm/xterm/css/xterm.css", "xterm.css"],
  ["@xterm/addon-fit/lib/addon-fit.js", "addon-fit.js"],
  ["@xterm/addon-webgl/lib/addon-webgl.js", "addon-webgl.js"]
];

for (const [from, to] of files) {
  copyFileSync(join(root, "node_modules", from), join(root, "src/vendor", to));
  console.log(`vendored src/vendor/${to}`);
}
