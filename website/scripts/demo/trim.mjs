// Strip only the TRAILING teardown run (alt-screen exit / terminal reset) so a
// looping player doesn't flash the restored shell. Walks from the end and stops
// at the first real frame, so mid-session mouse-mode toggles are never cut.
import fs from "node:fs";

const path = process.argv[2];
const lines = fs.readFileSync(path, "utf8").split("\n").filter(Boolean);
const header = lines[0];
const ev = lines.slice(1);
const RIS = String.fromCharCode(27) + "c"; // ESC c terminal reset, exit only

let cut = ev.length;
for (let i = ev.length - 1; i >= 0; i--) {
  let e;
  try {
    e = JSON.parse(ev[i]);
  } catch {
    continue;
  }
  const d = e[2] || "";
  if (d.includes("?1049l") || d.includes(RIS)) cut = i;
  else break;
}

fs.writeFileSync(path, [header, ...ev.slice(0, cut)].join("\n") + "\n");
console.log(`trimmed: kept ${cut} events (from ${ev.length})`);
