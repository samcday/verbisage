"""Expand the service template's literal %bindir% placeholder.

A D-Bus service file is parsed in two layers, so the executable token has to be
encoded for both. The bus first decodes the service value (backslash escapes,
including \\t, \\n and \\r), and then the D-Bus shell parser splits Exec without
running a shell. The literal double quotes in the template delimit one command
argument; inside them the command parser gives a backslash, double quote, dollar
sign and backtick their literal meaning only when backslash-escaped. Encoding
the whole token once for the command argument and then once for the service
value keeps spaces, quotes and backslashes inside the path from splitting or
reinterpreting the command.
"""

from pathlib import Path
import sys

# Pass 1: characters that keep their literal meaning inside the command
# parser's double-quoted argument only when backslash-escaped.
COMMAND_ESCAPE = {"\\": "\\\\", '"': '\\"', "$": "\\$", "`": "\\`"}
# Pass 2: service-value escapes decoded by the bus before command parsing.
# A backslash has to be doubled again; control whitespace has named escapes.
SERVICE_ESCAPE = {"\\": "\\\\", "\t": "\\t", "\n": "\\n", "\r": "\\r"}


def encode_executable(executable):
    """Encode one filesystem path for the quoted Exec argument."""
    if "\0" in executable:
        raise SystemExit("configure-service.py: bindir must not contain NUL")
    command = "".join(COMMAND_ESCAPE.get(char, char) for char in executable)
    return "".join(SERVICE_ESCAPE.get(char, char) for char in command)


def main():
    template = Path(sys.argv[1]).read_text(encoding="utf-8")
    service = template.replace("%bindir%", encode_executable(sys.argv[3]))
    Path(sys.argv[2]).write_text(service, encoding="utf-8")


if __name__ == "__main__":
    main()
