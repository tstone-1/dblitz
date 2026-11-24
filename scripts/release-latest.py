"""Print the gh --latest flag for a draft, given the repository's GraphQL response on stdin."""

import json
import re
import sys


def version(tag):
    match = re.fullmatch(r"v(\d{2})\.([1-9]|1[0-2])\.(0|[1-9]\d*)", tag)
    if not match:
        raise ValueError(f"Unexpected release tag: {tag!r}")
    return tuple(map(int, match.groups()))


if __name__ == "__main__":
    response = json.load(sys.stdin)
    if response.get("errors"):
        sys.exit("Could not read the repository's latest release")
    candidate = version(sys.argv[1])
    latest = response["data"]["repository"]["latestRelease"]
    print("true" if latest is None or candidate >= version(latest["tagName"]) else "false")
