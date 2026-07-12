// Crop a rendered demo GIF so the terminal sits flush, matching the website.
// Two things get trimmed:
//   - agg's own padding: 1 cell horizontal and 0.5 cell vertical on each side.
//   - oy's blank top row(s): the website drops these with translateY(-1 row),
//     so the GIF must too, else there's dead space above the tab bar.
// So the top loses (0.5 cell pad + dropTop cells) and the bottom loses just the
// 0.5 cell pad (the status bar is real content — keep it).
// Uses sharp (already a website dependency) — no ImageMagick/gifsicle needed,
// and it preserves every frame's delay and the loop flag.
import { renameSync } from "node:fs";
import sharp from "sharp";

const [gif, cols, rows, dropTopArg] = process.argv.slice(2);
if (!gif || !cols || !rows) {
  console.error("usage: crop-gif.mjs <gif> <cols> <rows> [dropTopRows=1]");
  process.exit(1);
}
const c = Number(cols);
const r = Number(rows);
const dropTop = dropTopArg === undefined ? 1 : Number(dropTopArg);

const m = await sharp(gif, { animated: true }).metadata();
const w = m.width;
const ph = m.pageHeight; // per-frame height
const cellW = w / (c + 2); // agg pads 1 cell horizontally per side
const cellH = ph / (r + 1); // agg pads 0.5 cell vertically per side

const left = Math.round(cellW);
const top = Math.round((0.5 + dropTop) * cellH);
const bottom = Math.round(0.5 * cellH);
const newW = w - 2 * left;
const newH = ph - top - bottom;

const tmp = `${gif}.crop.gif`;
await sharp(gif, { animated: true })
  .extract({ left, top, width: newW, height: newH })
  .gif()
  .toFile(tmp);
renameSync(tmp, gif);

console.log(
  `cropped ${gif}: ${w}x${ph} -> ${newW}x${newH} (${m.pages} frames, dropTop=${dropTop})`,
);
