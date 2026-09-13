"""Expand the service template's literal %bindir% placeholder."""

from pathlib import Path
import sys

template = Path(sys.argv[1]).read_text(encoding="utf-8")
service = template.replace("%bindir%", sys.argv[3])
Path(sys.argv[2]).write_text(service, encoding="utf-8")
