#!/usr/bin/env python3
"""Opt-in typed web-search smoke with shared approval/receipt/replay assertions."""
import importlib.util
from pathlib import Path
import sys

spec = importlib.util.spec_from_file_location("search_smoke", Path(__file__).with_name("smoke-search-process.py"))
smoke = importlib.util.module_from_spec(spec)
spec.loader.exec_module(smoke)

if __name__ == "__main__":
    sys.exit(smoke.main(typed=True))
