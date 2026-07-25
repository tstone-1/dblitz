import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

// Repo root, two levels up from src/lib.
const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..");

function readJson(file: string): Record<string, unknown> {
  return JSON.parse(readFileSync(join(root, file), "utf8")) as Record<string, unknown>;
}

function readText(file: string): string {
  return readFileSync(join(root, file), "utf8");
}

/**
 * Placeholder committed alongside the updater wiring so the config is complete
 * and reviewable before the signing key exists. This test is what stops it from
 * reaching a release: a build with a placeholder pubkey produces artifacts no
 * client can verify, and the failure would otherwise surface as "the update
 * silently never installs" on users' machines rather than here.
 */
const PUBKEY_PLACEHOLDER = "REPLACE_WITH_TAURI_SIGNER_PUBLIC_KEY";

/**
 * The updater's correctness depends on three files agreeing, and each fails
 * silently in its own way: a missing pubkey fails only on a user's machine, a
 * missing permission fails only at runtime in a packaged build, and a parallel
 * release matrix drops a platform from `latest.json` with nothing erroring.
 * These are cheap file assertions precisely because the real feedback loop is
 * a published release nobody can update from.
 */
describe("updater configuration", () => {
  const conf = readJson("src-tauri/tauri.conf.json");
  const bundle = conf.bundle as Record<string, unknown>;
  const updater = (conf.plugins as Record<string, unknown>).updater as Record<string, unknown>;

  it("emits updater artifacts from the bundler", () => {
    // Without this the release builds no .app.tar.gz/.nsis.zip/.AppImage and no
    // .sig, so tauri-action has nothing to put in latest.json and skips it
    // entirely — a release that looks fine and updates nobody.
    expect(bundle.createUpdaterArtifacts).toBe(true);
  });

  it("carries a real signing public key", () => {
    const pubkey = updater.pubkey;
    expect(typeof pubkey).toBe("string");
    expect(pubkey).not.toBe(PUBKEY_PLACEHOLDER);
    expect((pubkey as string).length).toBeGreaterThan(0);
  });

  it("fetches the manifest over https from this repo's releases", () => {
    const endpoints = updater.endpoints as string[];
    expect(Array.isArray(endpoints)).toBe(true);
    expect(endpoints.length).toBeGreaterThan(0);
    for (const endpoint of endpoints) {
      // Plain http would let a network attacker serve a manifest; the minisign
      // signature still gates installation, but https costs nothing here. Tauri
      // validates this while deserializing the config, so a plain-http endpoint
      // makes the packaged app panic on startup rather than merely warn.
      expect(endpoint.startsWith("https://")).toBe(true);
    }
    expect(endpoints).toContain(
      "https://github.com/tstone-1/dblitz/releases/latest/download/latest.json",
    );
  });

  it("does not ship the insecure-transport escape hatch", () => {
    // Only ever acceptable in a throwaway local test build (see BUILD.md's
    // local verification recipe), which is exactly why it is easy to leave
    // behind. It would let a network attacker serve the manifest.
    expect(updater.dangerousInsecureTransportProtocol).toBeUndefined();
  });

  it("grants the main window updater and restart permissions", () => {
    const capability = readJson("src-tauri/capabilities/default.json");
    const permissions = capability.permissions as string[];
    expect(permissions).toContain("updater:default");
    expect(permissions).toContain("process:allow-restart");
  });
});

/**
 * The release workflow is the other half of the updater, and its two
 * updater-specific settings both fail silently when absent.
 */
describe("release workflow updater wiring", () => {
  const workflow = readText(".github/workflows/release.yml");

  it("serializes the build matrix", () => {
    // tauri-action builds latest.json by read-modify-write against the release
    // asset: download the existing one, merge this platform's entries, delete
    // it, re-upload. dblitz has FOUR concurrent legs, two of which are macOS,
    // so parallel runs can both read the pre-merge state and the later upload
    // silently drops the earlier platform's entry. Nothing fails; the release
    // just quietly updates one platform.
    expect(workflow).toMatch(/max-parallel:\s*1/);
  });

  it("passes the signing key to the build", () => {
    // Without these, tauri-action logs "Signature not found for the updater
    // JSON. Skipping upload..." and SUCCEEDS. The release ships with installers
    // and no manifest, and the only symptom is users never updating.
    expect(workflow).toContain("TAURI_SIGNING_PRIVATE_KEY:");
    expect(workflow).toContain("TAURI_SIGNING_PRIVATE_KEY_PASSWORD:");
  });

  it("points the Windows manifest entry at the NSIS installer", () => {
    // Defaults to false, which would put the MSI in latest.json. An MSI update
    // on top of an NSIS install produces a second, parallel installation
    // instead of an upgrade.
    expect(workflow).toMatch(/updaterJsonPreferNsis:\s*true/);
  });
});
