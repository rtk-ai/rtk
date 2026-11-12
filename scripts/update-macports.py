#!/usr/bin/env python3
"""Update the source-based MacPorts port from a stable tagged source archive.

Requires Python 3.11 or newer for TOML parsing.
"""

import argparse
import hashlib
import io
from pathlib import Path
import re
import tarfile
import tomllib


VERSION = r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)"
SETUP = r"^(github\.setup[ \t]+rtk-ai[ \t]+rtk[ \t]+)(\S+)([ \t]+v[ \t]*)$"
BLOCK = r"(?:[^\n\\]|\\\n)*"


def stable_version(tag):
    if not re.fullmatch("v" + VERSION, tag):
        raise ValueError(f"Expected a stable vMAJOR.MINOR.PATCH tag, got {tag!r}")
    return tuple(int(part) for part in tag[1:].split("."))


def source_metadata(source, version):
    data = source.read_bytes()
    with tarfile.open(fileobj=io.BytesIO(data), mode="r:gz") as archive:
        def read_toml(name):
            try:
                member = archive.extractfile(f"rtk-{version}/{name}")
            except KeyError as error:
                raise ValueError(f"Source archive is missing {name}") from error
            if member is None:
                raise ValueError(f"Source archive has no regular {name} file")
            return tomllib.loads(member.read().decode("utf-8"))

        package = read_toml("Cargo.toml")["package"]
        if package.get("name") != "rtk" or package.get("version") != version:
            raise ValueError("Source package does not match the requested RTK version")
        crates = []
        for package in read_toml("Cargo.lock")["package"]:
            if "source" not in package:
                continue
            if package["source"] != "registry+https://github.com/rust-lang/crates.io-index":
                raise ValueError("Non-registry dependency needs manual port updates")
            name, crate_version = package["name"], package["version"]
            checksum = package.get("checksum", "")
            if not re.fullmatch(r"[A-Za-z0-9_-]+", name):
                raise ValueError(f"Invalid crate name: {name!r}")
            if not re.fullmatch(r"[A-Za-z0-9.+-]+", crate_version):
                raise ValueError(f"Invalid crate version: {crate_version!r}")
            if not re.fullmatch(r"[a-f0-9]{64}", checksum):
                raise ValueError(f"Invalid checksum for {name}")
            crates.append((name, crate_version, checksum))
    if not crates:
        raise ValueError("No registry crates found in Cargo.lock")
    checksums = {
        "rmd160": hashlib.new("ripemd160", data).hexdigest(),
        "sha256": hashlib.sha256(data).hexdigest(),
        "size": str(len(data)),
    }
    return checksums, crates


def replace_once(pattern, replacement, text):
    updated, count = re.subn(pattern, replacement, text, flags=re.MULTILINE)
    if count != 1:
        raise ValueError(f"Unexpected Portfile layout: {pattern!r} matched {count} times")
    return updated


def update_checksums(text, checksums):
    def replace_block(match):
        block = match.group()
        for key, value in checksums.items():
            block = replace_once(
                r"\b(" + key + r"[ \t]+)[^\s\\]+",
                lambda field: field[1] + value,
                block,
            )
        return block

    return replace_once(r"^checksums\b" + BLOCK, replace_block, text)


def update_portfile(portfile, source, tag):
    incoming = stable_version(tag)
    original = portfile.read_text()
    versions = re.findall(SETUP, original, re.MULTILINE)
    if len(versions) != 1:
        raise ValueError("Expected exactly one rtk-ai/rtk github.setup in the Portfile")
    current = stable_version("v" + versions[0][1])
    if incoming < current:
        raise ValueError(f"Refusing to downgrade {versions[0][1]} to {tag[1:]}")

    checksums, crates = source_metadata(source, tag[1:])
    updated = update_checksums(original, checksums)
    crate_lines = ["cargo.crates \\"]
    for index, (name, version, checksum) in enumerate(crates):
        continuation = " \\" if index < len(crates) - 1 else ""
        crate_lines.append(f"    {name:<32} {version:>14}  {checksum}{continuation}")
    updated = replace_once(
        r"^cargo\.crates\b" + BLOCK,
        lambda match: "\n".join(crate_lines),
        updated,
    )
    if incoming == current:
        if updated != original:
            raise ValueError("Source or crate checksums changed for an existing version; review manually")
        return False

    updated = replace_once(
        SETUP,
        lambda match: match[1] + tag[1:] + match[3],
        updated,
    )
    updated = re.sub(r"^(revision[ \t]+)\d+", r"\g<1>0", updated, flags=re.MULTILINE)
    portfile.write_text(updated)
    return True


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tag", required=True)
    parser.add_argument("--portfile", type=Path, required=True)
    parser.add_argument("--source", type=Path, required=True)
    args = parser.parse_args()
    try:
        changed = update_portfile(args.portfile, args.source, args.tag)
    except (ValueError, OSError, KeyError, tarfile.TarError) as error:
        parser.exit(1, f"MacPorts update failed: {error}\n")
    print("Updated Portfile" if changed else "Portfile is already up to date")


if __name__ == "__main__":
    main()
