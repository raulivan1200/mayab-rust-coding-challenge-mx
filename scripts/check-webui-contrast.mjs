#!/usr/bin/env node
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { resolve } from "node:path";

const root = resolve(fileURLToPath(new URL(".", import.meta.url)), "..");
const cssPath = resolve(root, "internal/webui/web/styles.css");
const css = readFileSync(cssPath, "utf8");

const minRatio = 4.5;
const prohibitedBackgrounds = [
  { name: "green", pattern: /background(?:-color)?\s*:[^;]*(?:var\(--verde\)|var\(--green\)|#00aa3c|#1be349|rgb\(0,\s*170,\s*60\)|rgb\(27,\s*227,\s*73\))/i },
  { name: "saturated blue", pattern: /background(?:-color)?\s*:[^;]*(?:#0072e3|#16a6ff|rgb\(0,\s*114,\s*227\)|rgb\(22,\s*166,\s*255\))/i },
];

const checks = [
  ["body", "#f4e9e1", "#000000"],
  ["card yellow", "#ffdb08", "#000000"],
  ["card orange", "#ff8e0a", "#000000"],
  ["card purple", "#c79dfc", "#000000"],
  ["card black", "#000000", "#ffffff"],
  ["benchmark panel", "#ff8e0a", "#000000"],
  ["table header", "#000000", "#ffffff"],
  ["table hover", "#ffdb08", "#000000"],
  ["cream panel", "#ffffff", "#000000"],
];

function hexToRgb(hex) {
  const clean = hex.replace("#", "");
  const full = clean.length === 3
    ? clean.split("").map((c) => c + c).join("")
    : clean;
  const value = Number.parseInt(full, 16);
  return [(value >> 16) & 255, (value >> 8) & 255, value & 255];
}

function channel(v) {
  const s = v / 255;
  return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
}

function luminance(hex) {
  const [r, g, b] = hexToRgb(hex).map(channel);
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

function contrast(bg, fg) {
  const a = luminance(bg);
  const b = luminance(fg);
  const lighter = Math.max(a, b);
  const darker = Math.min(a, b);
  return (lighter + 0.05) / (darker + 0.05);
}

const failures = [];

for (const blocked of prohibitedBackgrounds) {
  const match = css.match(blocked.pattern);
  if (match) {
    failures.push(`Prohibited ${blocked.name} background found: ${match[0].trim()}`);
  }
}

for (const [name, bg, fg] of checks) {
  const ratio = contrast(bg, fg);
  if (ratio < minRatio) {
    failures.push(`${name}: contrast ${ratio.toFixed(2)} is below ${minRatio} (${fg} on ${bg})`);
  }
}

if (!/\.benchmark-panel h2,[\s\S]*?color:\s*#000000;/.test(css)) {
  failures.push("benchmark-panel h2 must be explicitly black to avoid inherited blue headings.");
}

if (failures.length > 0) {
  console.error("Web UI contrast check failed:");
  for (const failure of failures) console.error(`- ${failure}`);
  process.exit(1);
}

console.log(`Web UI contrast check passed (${checks.length} contrast pairs, no prohibited backgrounds).`);
