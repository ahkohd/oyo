// Crop agg's built-in padding from a rendered demo GIF so the terminal sits
// flush. agg wraps the grid with 1 cell of horizontal and 0.5 cell of vertical
// padding on each side; given the terminal's cols/rows we strip exactly that.
// Uses sharp (already a website dependency) — no ImageMagick/gifsicle needed,
// and it preserves every frame's delay and the loop flag.
import { renameSync } from "node:fs";
import sharp from "sharp";

const [gif, cols, rows] = process.argv.slice(2);
if (!gif || !cols || !rows) {
  console.error("usage: crop-gif.mjs <gif> <cols> <rows>");
  process.exit(1);
}
const c = Number(cols);
const r = Number(rows);

const m = await sharp(gif, { animated: true }).metadata();
const w = m.width;
const ph = m.pageHeight; // per-frame height
const padX = Math.round(w / (c + 2)); // 1 cell each side
const padY = Math.round((0.5 * ph) / (r + 1)); // 0.5 cell each side

const tmp = `${gif}.crop.gif`;
await sharp(gif, { animated: true })
  .extract({ left: padX, top: padY, width: w - 2 * padX, height: ph - 2 * padY })
  .gif()
  .toFile(tmp);
renameSync(tmp, gif);

console.log(
  `cropped ${gif}: ${w}x${ph} -> ${w - 2 * padX}x${ph - 2 * padY} (${m.pages} frames)`,
);
