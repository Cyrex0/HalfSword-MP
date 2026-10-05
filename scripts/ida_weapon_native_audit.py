"""Read native functions from an isolated copy of the user's game IDB."""
import json
from pathlib import Path
import re

import ida_auto
import ida_funcs
import ida_hexrays
import ida_nalt
import ida_name
import ida_pro
import idautils

OUT = Path(r"D:\HalfswordMultiplayert\test-results\dev-feature-checks\weapon-catalogue-ida")

def main():
    OUT.mkdir(parents=True, exist_ok=True)
    ida_auto.auto_wait()
    # The existing database has damaged name-index entries. Direct function
    # lookup remains a separate proof gate; avoid treating an index scan as
    # evidence that every native symbol is present.
    names = []
    # These addresses come from existing native RVP analysis, not a guessed
    # gameplay health function. Resolve and record their actual IDB functions.
    exports = []
    for text in ("144A6E840", "1449F4230"):
        address = int(text, 16)
        function = ida_funcs.get_func(address)
        row = {"requested_address": hex(address), "function_found": bool(function)}
        if function:
            row.update(address=hex(function.start_ea), name=ida_funcs.get_func_name(function.start_ea))
            try:
                pseudocode = ida_hexrays.decompile(function.start_ea)
                if pseudocode:
                    file = OUT / (f"native-{function.start_ea:x}.c")
                    file.write_text(str(pseudocode), encoding="utf-8")
                    row["pseudocode_file"] = str(file)
                else:
                    row["decompile_error"] = "no pseudocode"
            except Exception as error:
                row["decompile_error"] = str(error)
        exports.append(row)
    digest = ida_nalt.retrieve_input_file_sha256()
    report = {"input_file": ida_nalt.get_input_file_path(),
              "input_sha256": digest.hex() if digest else None,
              "matching_native_names": names, "exports": exports,
              "scope": "two existing native RVP functions; name index omitted after damaged entries; Blueprint gameplay logic audited separately"}
    (OUT / "native-index.json").write_text(json.dumps(report, indent=2), encoding="utf-8")

try:
    main()
except Exception as error:
    OUT.mkdir(parents=True, exist_ok=True)
    (OUT / "error.txt").write_text(repr(error), encoding="utf-8")
finally:
    # The launcher opens only a copied IDB. The original remains untouched.
    ida_pro.qexit(0)
