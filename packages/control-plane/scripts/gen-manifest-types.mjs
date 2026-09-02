#!/usr/bin/env node
// Regenerates packages/control-plane/src/generated/manifest-schema.ts from
// schemas/manifest.v0.3.json (normative). Addendum 02a §1.4: the TypeScript
// view of the manifest is generated, never hand-rolled.

import { readFileSync, writeFileSync, mkdirSync } from "node:fs";
import { dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { compile } from "json-schema-to-typescript";

const root = dirname(dirname(dirname(dirname(fileURLToPath(import.meta.url)))));
const schemaPath = `${root}/schemas/manifest.v0.3.json`;
const outPath = `${root}/packages/control-plane/src/generated/manifest-schema.ts`;

const schema = JSON.parse(readFileSync(schemaPath, "utf8"));
const ts = await compile(schema, "Manifest", {
  additionalProperties: false,
  bannerComment: "/**\n * GENERATED FROM schemas/manifest.v0.3.json — do not edit (addendum 02a §1.4).\n * Regenerate: npm run gen:manifest-types.\n */"
});

mkdirSync(dirname(outPath), { recursive: true });
writeFileSync(outPath, ts);
console.log(`wrote ${outPath}`);
