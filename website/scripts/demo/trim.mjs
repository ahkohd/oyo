// Strip the TRAILING teardown so a looping player / GIF doesn't end on a blank
// restored screen. oy leaves the alternate screen exactly once, at quit, so we
// cut from that alt-screen-exit (or a terminal reset) to the end — that also
// drops any stray shell bytes emitted after it (e.g. a lone "0"). Everything
// before it, including mid-session mouse-mode toggles, is kept.
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
  if (d.includes("?1049l") || d.includes(RIS)) {
    cut = i; // cut from the teardown onward; keep scanning no further
    break;
  }
}

fs.writeFileSync(path, [header, ...ev.slice(0, cut)].join("\n") + "\n");
console.log(`trimmed: kept ${cut} events (from ${ev.length})`);
