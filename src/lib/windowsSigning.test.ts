import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

// Repo root, two levels up from src/lib.
const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..");

function readText(file: string): string {
  return readFileSync(join(root, file), "utf8");
}

function readJson(file: string): Record<string, unknown> {
  return JSON.parse(readText(file)) as Record<string, unknown>;
}

const release = readText(".github/workflows/release.yml");
const rehearsal = readText(".github/workflows/sign-rehearsal.yml");

/** The `build` job of release.yml, from its key to the next job's. */
const build = release.slice(release.indexOf("\n  build:"), release.indexOf("\n  publish:"));

/** The steps of a job, each from its `- ` line to the next one's. */
function steps(job: string): string[] {
  return job.split(/\n {6}- /).slice(1);
}

/**
 * The Windows release is signed with a certificate whose key cannot be
 * exported: what the workflow holds is a login. None of this can be run on a
 * development machine, and every part of it fails quietly when it is wrong: an
 * unsigned uninstaller builds green, a login passed to a leg that does not
 * sign is simply there, and an MSI that cannot be signed is uploaded like any
 * other file. These are file assertions because the real feedback loop is a
 * tag.
 */
describe("Windows signing configuration", () => {
  it("names the sign command in an overlay, not in the main config", () => {
    const overlay = readJson("src-tauri/tauri.signing.conf.json");
    const windows = (overlay.bundle as Record<string, unknown>).windows as Record<string, unknown>;
    // `%1` is the placeholder Tauri replaces with the file's path.
    expect(windows.signCommand).toEqual({ cmd: "sign-windows.cmd", args: ["%1"] });

    // In tauri.conf.json it would make every local Windows build ask for the
    // signing login.
    const main = readJson("src-tauri/tauri.conf.json");
    const mainWindows = (main.bundle as Record<string, unknown>).windows as
      | Record<string, unknown>
      | undefined;
    expect(mainWindows?.signCommand).toBeUndefined();
  });

  it("builds no MSI on Windows and leaves the other platforms alone", () => {
    // The signing client signs executables only, and a release with a signed
    // installer beside an unsigned package is worse than one without it.
    const windows = readJson("src-tauri/tauri.windows.conf.json");
    expect((windows.bundle as Record<string, unknown>).targets).toEqual(["nsis"]);
    // dmg, AppImage, deb and rpm still come from "all".
    const main = readJson("src-tauri/tauri.conf.json");
    expect((main.bundle as Record<string, unknown>).targets).toBe("all");
  });

  it("finds the signing script through the environment, not beside the wrapper", () => {
    // makensis starts the wrapper by name through PATH from its own folder,
    // where %~dp0 is that folder: the uninstaller then stays unsigned and the
    // build stays green.
    const commands = readText("scripts/sign-windows.cmd")
      .split(/\r?\n/)
      .filter((line) => !/^\s*rem\b/i.test(line));
    expect(commands.length).toBeGreaterThan(3);
    expect(commands.join("\n")).not.toContain("%~dp0");
    expect(commands.join("\n")).toContain('"%DBLITZ_SIGN_SCRIPT%" %1');
    for (const workflow of [build, rehearsal]) {
      expect(workflow).toContain(
        "DBLITZ_SIGN_SCRIPT: ${{ github.workspace }}/scripts/sign-windows.ps1",
      );
    }
  });

  it("signs into a separate folder and copies back", () => {
    // ssign replaces a file by rename, which fails while Tauri still has the
    // file open.
    const script = readText("scripts/sign-windows.ps1");
    expect(script).toContain("ssign --verbose --output-dir $signed $Path");
    expect(script).toContain("Copy-Item -LiteralPath $copy -Destination $Path -Force");
  });
});

describe("release workflow Windows signing", () => {
  it("reads the build job", () => {
    // Emptiness control for the slice every assertion below reads.
    expect(build).toContain("Build and upload artifacts");
    expect(steps(build).length).toBeGreaterThan(10);
  });

  it("marks exactly one leg as signing, and it is the Windows one", () => {
    const legs = build.split(/\n {10}- os: /).slice(1);
    expect(legs.length).toBe(4);
    const signing = legs.filter((leg) => /^\s*signs-windows: true$/m.test(leg));
    expect(signing.length).toBe(1);
    expect(signing[0].startsWith("windows-latest")).toBe(true);
    for (const leg of legs) {
      expect(leg).toMatch(/^\s*signs-windows: (true|false)$/m);
    }
  });

  it("gives the signing environment to that leg only", () => {
    expect(build).toContain("environment: ${{ matrix.signs-windows && 'signing' || '' }}");
  });

  it("hands the Certum login to no step that runs on another leg", () => {
    const withLogin = steps(build).filter((step) => step.includes("secrets.CERTUM_"));
    // The check step and the build step.
    expect(withLogin.length).toBe(2);
    for (const step of withLogin) {
      const guardedStep = /^ {8}if: matrix\.signs-windows$/m.test(step);
      const lines = step.split("\n").filter((line) => line.includes("secrets.CERTUM_"));
      const guardedLines = lines.every((line) =>
        /\$\{\{ matrix\.signs-windows && secrets\.CERTUM_\w+ \|\| '' \}\}/.test(line),
      );
      expect(guardedStep || guardedLines).toBe(true);
    }
    // And nowhere outside the build job.
    expect(release.split("secrets.CERTUM_").length).toBe(build.split("secrets.CERTUM_").length);
  });

  it("applies the signing overlay on that leg only, with a path built in the step", () => {
    // `github.workspace` is empty inside a matrix, so a path put together
    // there starts at the drive's root.
    expect(build).toContain(
      "${{ matrix.signs-windows && format('--config {0}/src-tauri/tauri.signing.conf.json', github.workspace) || '' }}",
    );
    const matrix = build.slice(build.indexOf("matrix:"), build.indexOf("runs-on:"));
    expect(matrix).toContain("signs-windows: true");
    expect(matrix.split("\n").filter((l) => !l.trim().startsWith("#")).join("\n")).not.toContain(
      "github.workspace",
    );
  });

  it("leaves no token in the checkout of the job that builds the signing client", () => {
    const checkout = steps(build)[0];
    expect(checkout).toContain("actions/checkout@");
    expect(checkout).toContain("persist-credentials: false");
  });

  it("removes the signing session and prints the log even when the build failed", () => {
    for (const name of ["Remove the Windows signing session", "Show what the signing client said"]) {
      const step = steps(build).find((s) => s.startsWith(`name: ${name}\n`));
      expect(step).toBeDefined();
      expect(step).toMatch(/^ {8}if: always\(\) && matrix\.signs-windows$/m);
    }
  });

  it("reads the signatures back before the portable exe is uploaded", () => {
    const names = steps(build).map((step) => step.split("\n")[0]);
    const verify = names.indexOf("name: Verify the Windows build is signed");
    const upload = names.indexOf("name: Upload portable exe");
    const built = names.indexOf("name: Build and upload artifacts");
    expect(built).toBeGreaterThan(-1);
    expect(verify).toBeGreaterThan(built);
    expect(upload).toBeGreaterThan(verify);

    const step = steps(build)[verify];
    expect(step).toMatch(/^ {8}if: matrix\.signs-windows$/m);
    expect(step).toContain("$signer = 'Open Source Developer Timo Stein'");
    // The installer and the portable exe, then what the installer leaves on
    // the disk, the uninstaller among it.
    expect(step).toContain("scripts/verify-signature.ps1 -Signer $signer -Path $setup, $portable");
    expect(step).toContain("'uninstall.exe'");
    expect(step).toContain("scripts/verify-signature.ps1 -Signer $signer -Path $installed.FullName");
    // The upload takes the file the step read.
    expect(steps(build)[upload]).toContain("src-tauri/target/release/dblitz.exe");
    expect(step).toContain('$portable = "src-tauri/target/release/$name.exe"');
  });
});

describe("everything that signs logs in one at a time, with the same client", () => {
  it("shares one concurrency group that cancels nothing", () => {
    expect(build).toMatch(
      /concurrency:\n {6}group: \$\{\{ matrix\.signs-windows && 'certum-signing' \|\| format\(/,
    );
    expect(rehearsal).toMatch(/^concurrency:\n {2}group: certum-signing\n {2}cancel-in-progress: false$/m);
    expect(build).toMatch(/group: .*certum-signing.*\n {6}cancel-in-progress: false/);
  });

  it("builds the same pinned commit of the signing client in both workflows", () => {
    const pins = [build, rehearsal].map((text) => /SSIGN_REV: ([0-9a-f]{40})$/m.exec(text)?.[1]);
    expect(pins[0]).toBeDefined();
    expect(pins[0]).toBe(pins[1]);
    for (const text of [build, rehearsal]) {
      expect(text).toContain(
        "cargo install --locked --git https://github.com/Le-Syl21/ssign --rev $env:SSIGN_REV --root $root ssign",
      );
    }
  });

  it("keeps the rehearsal away from pull requests and inside the environment", () => {
    expect(rehearsal).toMatch(/^on:\n {2}workflow_dispatch:$/m);
    expect(rehearsal).not.toContain("pull_request");
    expect(rehearsal).toMatch(/^ {4}environment: signing$/m);
    expect(rehearsal).toContain("persist-credentials: false");
    expect(rehearsal).toMatch(/- name: Remove the cached session\n {8}if: always\(\)/);
  });
});
