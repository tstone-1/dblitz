"""Verify downloaded release assets against the signatures an installed client will use."""

import base64
import json
import subprocess
import sys
from pathlib import Path
from tempfile import TemporaryDirectory


def verify(directory: Path, version: str, config: Path) -> None:
    manifest = json.loads((directory / "latest.json").read_bytes())
    if manifest["version"] != version:
        raise ValueError(f"latest.json version must be {version}")
    assets = {asset["name"]: asset for asset in json.loads((directory / "assets.json").read_bytes())}
    platforms = manifest["platforms"]
    required = {
        "darwin-aarch64": f"dblitz_{version}_aarch64.app.tar.gz",
        "darwin-x86_64": f"dblitz_{version}_x64.app.tar.gz",
        "windows-aarch64": f"dblitz_{version}_arm64-setup.exe",
        "windows-x86_64": f"dblitz_{version}_x64-setup.exe",
        "linux-x86_64": f"dblitz_{version}_amd64.AppImage",
    }
    if missing := required.keys() - platforms.keys():
        raise ValueError(f"missing updater platforms: {sorted(missing)}")
    expected = dict(required)
    for platform, name in required.items():
        suffix = {"darwin": "app", "windows": "nsis", "linux": "appimage"}[platform.split("-")[0]]
        expected[f"{platform}-{suffix}"] = name
    expected["linux-x86_64-deb"] = f"dblitz_{version}_amd64.deb"
    expected["linux-x86_64-rpm"] = f"dblitz-{version}-1.x86_64.rpm"

    with TemporaryDirectory() as scratch:
        public_key = Path(scratch) / "minisign.pub"
        signature = Path(scratch) / "artifact.minisig"
        key = json.loads(config.read_bytes())["plugins"]["updater"]["pubkey"]
        public_key.write_bytes(base64.b64decode(key, validate=True))
        verified = set()
        for platform, entry in platforms.items():
            name = expected[platform]
            asset = assets[name]
            if entry["url"] not in (asset["url"], asset["browser_download_url"]):
                raise ValueError(f"{platform} must point to the draft's {name}")
            identity = (name, entry["signature"])
            if identity in verified:
                continue
            signature.write_bytes(base64.b64decode(entry["signature"], validate=True))
            # Minisign verifies both the downloaded bytes and the trusted comment.
            subprocess.run(["minisign", "-V", "-q", "-p", str(public_key), "-x", str(signature),
                            "-m", str(directory / name)], check=True)
            trusted = signature.read_text(encoding="utf-8").splitlines()[2]
            versions = [field for field in trusted.removeprefix("trusted comment: ").split("\t")
                        if field.startswith("version:")]
            if versions != [f"version:{version}"]:
                raise ValueError(f"{name} signature has the wrong release version")
            verified.add(identity)
    print(f"[OK] verified {len(verified)} signed payloads for {len(platforms)} platform entries")


if __name__ == "__main__":
    verify(Path(sys.argv[1]), sys.argv[2], Path(sys.argv[3]))
