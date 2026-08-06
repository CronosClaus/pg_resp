#!/usr/bin/env python3
"""Validate META.json against the PGXN Meta Spec v1.0.0.

The spec (https://pgxn.org/meta/spec.txt) is prose, not a JSON Schema, and the
reference validators are Perl (PGXN::Meta::Validator) and Rust (pgxn_meta) — neither
available here. So the spec's rules are encoded directly, with the section each one
comes from, and the license list is transcribed from the spec's own License Strings
table rather than assumed.

This exists so the check is reproducible and reviewable instead of being a claim in a
commit message. Run before any PGXN upload: the upload page publishes immediately.
"""
import json, re, sys

# Spec: "License Strings" table, List representation.
LICENSES = {
    "agpl_3","apache_1_1","apache_2_0","artistic_1","artistic_2","bsd","freebsd",
    "gfdl_1_2","gfdl_1_3","gpl_1","gpl_2","gpl_3","lgpl_2_1","lgpl_3_0","mit",
    "mozilla_1_0","mozilla_1_1","openssl","perl_5","postgresql","qpl_1_0","ssleay",
    "sun","zlib","open_source","restricted","unrestricted","unknown",
}
REQUIRED = ["name","abstract","version","maintainer","license","provides","meta-spec"]
TERM = re.compile(r'^[^\s:][^:]*$')                     # Term: no colon, no leading space
SEMVER = re.compile(r'^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$')  # strict dotted-integer

def main(path):
    errs, warns = [], []
    m = json.load(open(path))

    for k in REQUIRED:
        if k not in m:
            errs.append(f"missing required field: {k}")
    if errs:
        return report(errs, warns)

    if not TERM.match(m["name"]):
        errs.append(f"name {m['name']!r} is not a valid Term")
    if not SEMVER.match(m["version"]):
        errs.append(f"version {m['version']!r} is not a strict semantic version")

    lic = m["license"]
    for l in ([lic] if isinstance(lic, str) else lic if isinstance(lic, list) else []):
        if l not in LICENSES:
            errs.append(f"license {l!r} is not a spec License String")
    if isinstance(lic, dict):
        for name, uri in lic.items():
            if not str(uri).startswith(("http://","https://")):
                errs.append(f"license map {name!r} value is not a URI")

    mnt = m["maintainer"]
    if not (isinstance(mnt, str) or (isinstance(mnt, list) and mnt and all(isinstance(x,str) for x in mnt))):
        errs.append("maintainer must be a string or non-empty list of strings")

    ms = m["meta-spec"]
    if not isinstance(ms, dict) or ms.get("version") != "1.0.0":
        errs.append("meta-spec.version must be '1.0.0'")

    prov = m["provides"]
    if not isinstance(prov, dict) or not prov:
        errs.append("provides must be a non-empty map")
    else:
        for ext, d in prov.items():
            if not TERM.match(ext):
                errs.append(f"provides key {ext!r} is not a valid Term")
            for req in ("file","version"):
                if req not in d:
                    errs.append(f"provides.{ext} missing required {req}")
            if "version" in d and not SEMVER.match(d["version"]):
                errs.append(f"provides.{ext}.version {d['version']!r} is not a strict semantic version")

    # Cross-checks the spec does not require but a mismatch is always a mistake.
    for ext, d in (prov.items() if isinstance(prov, dict) else []):
        if d.get("version") and d["version"] != m["version"]:
            warns.append(f"provides.{ext}.version ({d['version']}) != distribution version ({m['version']})")

    return report(errs, warns)

def report(errs, warns):
    for w in warns: print(f"WARN  {w}")
    for e in errs:  print(f"ERROR {e}")
    print(f"\n{'INVALID' if errs else 'VALID'} — {len(errs)} error(s), {len(warns)} warning(s)")
    return 1 if errs else 0

if __name__ == "__main__":
    sys.exit(main(sys.argv[1] if len(sys.argv) > 1 else "META.json"))
