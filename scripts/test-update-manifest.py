"""Exercise the publish gate with temporary payloads and throwaway signing keys."""

import base64
import json
import runpy
import subprocess
import unittest
from pathlib import Path
from tempfile import TemporaryDirectory

verify = runpy.run_path(str(Path(__file__).with_name("verify-update-manifest.py")))["verify"]


class UpdateManifestTest(unittest.TestCase):
    def setUp(self):
        temporary = TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.config = self.root / "config.json"
        self.version = "26.12.0"
        self.generated_key("sample")
        assets = []
        self.platforms = {}
        for platform, suffix in [
            ("darwin-aarch64", "aarch64.app.tar.gz"), ("darwin-x86_64", "x64.app.tar.gz"),
            ("windows-aarch64", "arm64-setup.exe"), ("windows-x86_64", "x64-setup.exe"),
            ("linux-x86_64", "amd64.AppImage"),
        ]:
            name = f"dblitz_{self.version}_{suffix}"
            path = self.root / name
            path.write_text(f"synthetic {platform}", encoding="utf-8")
            subprocess.run(["minisign", "-S", "-s", str(self.root / "sample.key"), "-m", str(path),
                            "-t", f"version:{self.version}"], check=True, capture_output=True)
            url = f"https://api.example.invalid/assets/{len(assets) + 1}"
            assets.append({"name": name, "url": url, "browser_download_url": f"https://example.invalid/{name}"})
            signature = base64.b64encode(Path(f"{path}.minisig").read_bytes()).decode()
            self.platforms[platform] = {"url": url, "signature": signature}
        (self.root / "assets.json").write_text(json.dumps(assets), encoding="utf-8")

    def generated_key(self, name):
        public = self.root / f"{name}.pub"
        subprocess.run(["minisign", "-G", "-W", "-p", str(public), "-s", str(self.root / f"{name}.key")],
                       check=True, capture_output=True)
        public_key = base64.b64encode(public.read_bytes()).decode()
        self.config.write_text(json.dumps({"plugins": {"updater": {"pubkey": public_key}}}), encoding="utf-8")

    def check(self, manifest_version=None):
        manifest = {"version": manifest_version or self.version, "platforms": self.platforms}
        (self.root / "latest.json").write_text(json.dumps(manifest), encoding="utf-8")
        verify(self.root, self.version, self.config)

    def test_accepts_signed_payloads(self):
        self.check()

    def test_accepts_the_same_key_with_another_comment_and_newlines(self):
        config = json.loads(self.config.read_bytes())
        key = config["plugins"]["updater"]
        body = base64.b64decode(key["pubkey"]).splitlines()[1]
        key["pubkey"] = base64.b64encode(b"untrusted comment: re-exported\r\n" + body + b"\r\n").decode()
        self.config.write_bytes(json.dumps(config).encode())
        self.check()

    def test_rejects_another_key(self):
        self.generated_key("other")
        with self.assertRaises(subprocess.CalledProcessError):
            self.check()

    def test_rejects_a_swapped_platform_with_its_valid_signature(self):
        self.platforms["windows-x86_64"] = self.platforms["windows-aarch64"]
        with self.assertRaisesRegex(ValueError, "windows-x86_64"):
            self.check()

    def test_rejects_a_missing_platform(self):
        del self.platforms["darwin-aarch64"]
        with self.assertRaisesRegex(ValueError, "missing updater platforms"):
            self.check()

    def test_rejects_another_manifest_version(self):
        with self.assertRaisesRegex(ValueError, "latest.json version"):
            self.check("26.11.0")

    def test_rejects_changed_payload_bytes(self):
        (self.root / f"dblitz_{self.version}_x64-setup.exe").write_bytes(b"altered synthetic payload")
        with self.assertRaises(subprocess.CalledProcessError):
            self.check()

    def test_rejects_a_valid_signature_for_a_different_release(self):
        path = self.root / f"dblitz_{self.version}_x64-setup.exe"
        subprocess.run(["minisign", "-S", "-s", str(self.root / "sample.key"), "-m", str(path),
                        "-t", "version:26.11.0"], check=True, capture_output=True)
        self.platforms["windows-x86_64"]["signature"] = base64.b64encode(Path(f"{path}.minisig").read_bytes()).decode()
        with self.assertRaisesRegex(ValueError, "wrong release version"):
            self.check()


if __name__ == "__main__":
    unittest.main()
