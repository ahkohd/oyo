// Trim both ends of a recording so a looping player / GIF opens and closes on
// the UI, never on a blank alt-screen:
//   - Trailing teardown: cut from the alternate-screen exit (or terminal reset)
//     to the end. oy leaves the alt-screen exactly once, at quit; this also
//     drops any stray shell bytes after it (e.g. a lone "0").
//   - Leading startup blank: oy takes a moment (cold start can be ~2s) to paint
//     its first frame, during which the recording shows the empty background.
//     Compress that idle so playback opens on the UI.
import fs from "node:fs";

const path = process.argv[2];
const lines = fs.readFileSync(path, "utf8").split("\n").filter(Boolean);
const header = lines[0];
let events = lines.slice(1).map((l) => JSON.parse(l));
const before = events.length;
const RIS = String.fromCharCode(27) + "c"; // ESC c terminal reset, exit only

// 1. Trailing teardown.
let cut = events.length;
for (let i = events.length - 1; i >= 0; i--) {
  const d = events[i][2] || "";
  if (d.includes("?1049l") || d.includes(RIS)) {
    cut = i;
    break;
  }
}
events = events.slice(0, cut);

// 2. Leading startup blank. Find oy's first substantial paint (a full-screen
// redraw is thousands of bytes; setup escapes are tiny), then collapse it — and
// all the setup before it — to t=0, so the very first rendered frame is the
// painted UI rather than the empty background. Rebase the rest by the same
// amount to preserve the choreography's timing.
let firstPaint = -1;
for (let i = 0; i < events.length; i++) {
  if ((events[i][2] || "").length >= 500) {
    firstPaint = i;
    break;
  }
}
if (firstPaint > 0) {
  const paintT = events[firstPaint][0];
  for (let i = 0; i <= firstPaint; i++) events[i][0] = 0;
  for (let i = firstPaint + 1; i < events.length; i++) {
    events[i][0] = Math.max(0, events[i][0] - paintT);
  }
}

fs.writeFileSync(
  path,
  [header, ...events.map((e) => JSON.stringify(e))].join("\n") + "\n",
);
console.log(
  `trimmed: kept ${events.length} events (from ${before}); lead paint at #${firstPaint}`,
);
