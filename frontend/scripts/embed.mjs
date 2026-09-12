import { readFileSync, writeFileSync } from "node:fs";
import { createHash } from "node:crypto";
import { gzipSync } from "node:zlib";
const html = readFileSync(new URL("../dist/index.html", import.meta.url));
const scripts = [
  ...html.toString().matchAll(/<script\b[^>]*>([\s\S]*?)<\/script>/g),
];
if (scripts.length === 0) throw new Error("No embedded scripts found");
const hashes = scripts
  .map((m) => `'sha256-${createHash("sha256").update(m[1]).digest("base64")}'`)
  .join(" ");
writeFileSync(
  new URL("../dist/csp.txt", import.meta.url),
  `default-src 'none'; script-src ${hashes}; style-src 'unsafe-inline'; img-src 'self' data:; font-src 'self' data:; connect-src 'self'; base-uri 'none'; frame-ancestors 'self'; form-action 'self'`,
);
writeFileSync(
  new URL("../dist/index.html.gz", import.meta.url),
  gzipSync(html, { level: 9 }),
);
