const fs = require("node:fs");
const path = require("node:path");

function isMuslLinux() {
  if (process.platform !== "linux") {
    return false;
  }

  if (typeof process.report?.getReport === "function") {
    const report = process.report.getReport();
    return !report?.header?.glibcVersionRuntime;
  }

  return false;
}

function packageForCurrentPlatform() {
  if (process.platform === "darwin" && process.arch === "arm64") {
    return "@ahkohd/oyo-darwin-arm64";
  }

  if (process.platform === "darwin" && process.arch === "x64") {
    return "@ahkohd/oyo-darwin-x64";
  }

  if (process.platform === "linux" && process.arch === "x64") {
    if (isMuslLinux()) {
      throw new Error(
        "native musl Linux binaries are not published yet (expected glibc Linux x64/arm64)"
      );
    }

    return "@ahkohd/oyo-linux-x64-gnu";
  }

  if (process.platform === "linux" && process.arch === "arm64") {
    if (isMuslLinux()) {
      throw new Error(
        "native musl Linux binaries are not published yet (expected glibc Linux x64/arm64)"
      );
    }

    return "@ahkohd/oyo-linux-arm64-gnu";
  }

  throw new Error(
    `unsupported platform ${process.platform}/${process.arch}; supported: darwin-arm64, darwin-x64, linux-x64-gnu, linux-arm64-gnu`
  );
}

function resolveBinary(binaryName) {
  const pkgName = packageForCurrentPlatform();
  let pkgJsonPath;
  try {
    pkgJsonPath = require.resolve(`${pkgName}/package.json`);
  } catch {
    throw new Error(
      `missing prebuilt package ${pkgName}; reinstall @ahkohd/oyo to fetch optional platform binaries`
    );
  }

  const packageDir = path.dirname(pkgJsonPath);
  const binaryPath = path.join(packageDir, "bin", binaryName);

  if (!fs.existsSync(binaryPath)) {
    throw new Error(`missing ${binaryName} in ${pkgName}`);
  }

  return binaryPath;
}

module.exports = {
  resolveBinary,
};
