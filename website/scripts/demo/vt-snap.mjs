// Minimal VT screen reconstructor: replays an asciicast's output up to a given
// time and prints the visible text grid. Handles CUP/erase/CR/LF; ignores SGR.
import fs from "node:fs";

const [castPath, atArg] = process.argv.slice(2);
const at = atArg ? parseFloat(atArg) : Infinity;
const lines = fs.readFileSync(castPath, "utf8").split("\n").filter(Boolean);
const hdr = JSON.parse(lines[0]);
const COLS = hdr.width, ROWS = hdr.height;
const grid = Array.from({ length: ROWS }, () => Array(COLS).fill(" "));
let cr = 0, cc = 0;
const put = (ch) => { if (cr < ROWS && cc < COLS) grid[cr][cc] = ch; cc++; if (cc >= COLS) { cc = COLS - 1; } };

let data = "";
for (let i = 1; i < lines.length; i++) {
  const e = JSON.parse(lines[i]);
  if (e[1] === "o" && e[0] <= at) data += e[2];
}

for (let i = 0; i < data.length; i++) {
  const c = data[i];
  if (c === "\x1b") {
    if (data[i + 1] === "[") {
      let j = i + 2, params = "";
      while (j < data.length && /[0-9;?]/.test(data[j])) params += data[j++];
      const cmd = data[j];
      const n = params.split(";").map((x) => parseInt(x || "0", 10));
      if (cmd === "H" || cmd === "f") { cr = (n[0] || 1) - 1; cc = (n[1] || 1) - 1; }
      else if (cmd === "A") cr = Math.max(0, cr - (n[0] || 1));
      else if (cmd === "B") cr = Math.min(ROWS - 1, cr + (n[0] || 1));
      else if (cmd === "C") cc = Math.min(COLS - 1, cc + (n[0] || 1));
      else if (cmd === "D") cc = Math.max(0, cc - (n[0] || 1));
      else if (cmd === "J") { const m = n[0] || 0; if (m === 2) for (let r = 0; r < ROWS; r++) grid[r].fill(" "); }
      else if (cmd === "K") { const m = n[0] || 0; if (m === 0) for (let x = cc; x < COLS; x++) grid[cr][x] = " "; else if (m === 2) grid[cr].fill(" "); }
      i = j;
    } else if (data[i + 1] === "]") { // OSC ... BEL/ST
      let j = i + 2; while (j < data.length && data[j] !== "\x07" && !(data[j] === "\x1b" && data[j + 1] === "\\")) j++;
      i = data[j] === "\x07" ? j : j + 1;
    } else { i += 1; }
  } else if (c === "\r") cc = 0;
  else if (c === "\n") { cr = Math.min(ROWS - 1, cr + 1); }
  else if (c === "\b") cc = Math.max(0, cc - 1);
  else if (c >= " ") put(c);
}

console.log("--- screen @ t=" + (at === Infinity ? "end" : at) + " (" + COLS + "x" + ROWS + ") ---");
console.log(grid.map((r) => r.join("").replace(/\s+$/, "")).join("\n"));
