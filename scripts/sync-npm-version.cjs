#!/usr/bin/env node

const fs = require("node:fs");
const path = require("node:path");

const ROOT = path.resolve(__dirname, "..");
const CARGO_TOML = path.join(ROOT, "Cargo.toml");
const ROOT_PACKAGE = path.join(ROOT, "package.json");
const PLATFORM_PACKAGES = [
  path.join(ROOT, "npm", "darwin-arm64", "package.json"),
  path.join(ROOT, "npm", "darwin-x64", "package.json"),
  path.join(ROOT, "npm", "linux-arm64-gnu", "package.json"),
  path.join(ROOT, "npm", "linux-x64-gnu", "package.json"),
];

const PLATFORM_DEPENDENCIES = [
  "@ahkohd/oyo-darwin-arm64",
  "@ahkohd/oyo-darwin-x64",
  "@ahkohd/oyo-linux-arm64-gnu",
  "@ahkohd/oyo-linux-x64-gnu",
];

function readJson(filePath) {
  return JSON.parse(fs.readFileSync(filePath, "utf8"));
}

function writeJson(filePath, value) {
  fs.writeFileSync(filePath, `${JSON.stringify(value, null, 2)}\n`);
}

function readVersionFromCargoToml() {
  const contents = fs.readFileSync(CARGO_TOML, "utf8");
  const match = contents.match(/^version\s*=\s*"([^"]+)"/m);
  if (!match) {
    throw new Error("unable to read version from Cargo.toml");
  }

  return match[1];
}

function main() {
  const requestedVersion = process.argv[2];
  const version = requestedVersion || readVersionFromCargoToml();

  const root = readJson(ROOT_PACKAGE);
  root.version = version;
  root.optionalDependencies = root.optionalDependencies || {};
  for (const name of PLATFORM_DEPENDENCIES) {
    root.optionalDependencies[name] = version;
  }
  writeJson(ROOT_PACKAGE, root);

  for (const packagePath of PLATFORM_PACKAGES) {
    const pkg = readJson(packagePath);
    pkg.version = version;
    writeJson(packagePath, pkg);
  }

  process.stdout.write(`synced npm package versions to ${version}\n`);
}

main();
