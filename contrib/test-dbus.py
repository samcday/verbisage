#!/usr/bin/python3
"""Exercise the real daemon on a private bus; no desktop or network is used."""
import argparse
from pathlib import Path
import subprocess
import tempfile
import time

from gi.repository import Gio, GLib

NAME = "org.verbisage.Dictionary"
PATH = "/org/verbisage/Dictionary"
IFACE = "org.verbisage.Dictionary1"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--daemon", type=Path, required=True)
    parser.add_argument("--patricia-dict", type=Path)
    args = parser.parse_args()
    connection = Gio.bus_get_sync(Gio.BusType.SESSION, None)

    def call(method, signature, values):
        result = connection.call_sync(NAME, PATH, IFACE, method,
                                      GLib.Variant(signature, values), None,
                                      Gio.DBusCallFlags.NONE, 5000, None)
        return result.unpack()[0]

    def expect_error(method, signature, values, fragment):
        try:
            call(method, signature, values)
        except GLib.Error as error:
            assert fragment in str(error), str(error)
        else:
            raise AssertionError(f"{method} unexpectedly succeeded")

    with tempfile.TemporaryDirectory(prefix="verbisage-dbus-test-") as directory:
        directory = Path(directory)
        # A frequency-bearing fixture verifies ranking and limits as well as
        # dictionary presence; this test does not depend on a user's config.
        (directory / "en_US.dic").write_text(
            "hello 10\nhelp 8\nhelium 6\n" + "\n".join(f"hword{i:03d} 1" for i in range(150)))
        config = directory / "config.toml"
        if args.patricia_dict:
            (directory / "en_US.dict").symlink_to(args.patricia_dict.resolve())
            config.write_text('backend = "patricia"\n[backends.patricia]\ntype = "patricia"\nsystem_dir = "' + str(directory) + '"\n')
        command = [str(args.daemon.resolve()), "--mode", "dbus", "--config",
                   str(config), "--backend", "patricia" if args.patricia_dict else "file",
                   "--system-data-dir", str(directory), "--user-dict", ""]
        with (directory / "daemon.log").open("w+") as log:
            process = subprocess.Popen(command, stdout=log, stderr=log)
            try:
                for _ in range(100):
                    if process.poll() is not None:
                        log.seek(0)
                        raise AssertionError(log.read())
                    owner = connection.call_sync("org.freedesktop.DBus", "/org/freedesktop/DBus",
                                                 "org.freedesktop.DBus", "NameHasOwner",
                                                 GLib.Variant("(s)", (NAME,)), None,
                                                 Gio.DBusCallFlags.NONE, 1000, None)
                    if owner.unpack()[0]:
                        break
                    time.sleep(0.05)
                else:
                    raise AssertionError("daemon did not acquire its bus name")

                completed = call("Complete", "(sus)", ("helo", 6, "en_US"))
                assert completed[0][0] == "hello", completed
                assert call("Complete", "(sus)", ("helo", 1, "en_US")) == completed[:1]
                assert all(0.0 <= score <= 1.0 for _, score in completed)
                assert call("Complete", "(sus)", ("helo", 0, "en_US")) == []
                assert call("Complete", "(sus)", ("", 6, "en_US")) == []
                assert call("Complete", "(sus)", ("two words", 6, "en_US")) == []
                assert all(word != "hello" for word, _ in call("Complete", "(sus)", ("hello", 6, "en_US")))
                expect_error("Complete", "(sus)", ("x" * 129, 6, "en_US"), "completion word is too large")
                expect_error("Complete", "(sus)", ("helo", 6, "../invalid"), "invalid language tag")
                expect_error("Complete", "(sus)", ("helo", 6, "zz_ZZ"), "no dictionary loaded")
                assert call("IsCorrect", "(ss)", ("hello", "en_US")) is True
                assert call("IsCorrect", "(ss)", ("zzinvalidwordzz", "en_US")) is False
                assert "hello" in call("Suggest", "(sus)", ("helo", 5, "en_US"))
                rows = call("QueryLimited", "(asasuusu)", (["hel"], [], 0, 0, "en_US", 2))
                assert 0 < len(rows) <= 2 and all(word.startswith("hel") for word, _ in rows), rows
                assert call("QueryLimited", "(asasuusu)", (["h"], [], 0, 0, "en_US", 0)) == []
                expect_error("QueryLimited", "(asasuusu)", (["h"] * 17, [], 0, 0, "en_US", 1),
                             "completion query is too large")
                expect_error("QueryLimited", "(asasuusu)", (["h"], [], 0, 0, "../../invalid", 1),
                             "invalid language tag")
                expect_error("QueryLimited", "(asasuusu)", (["h"], [], 0, 0, "zz_ZZ", 1),
                             "no dictionary loaded")
                if args.patricia_dict:
                    assert call("IsCorrect", "(ss)", ("running", "en_US")) is True
                    assert len(call("Predict", "(asus)", (["hello"], 3, "en_US"))) <= 3
                    assert call("Suggest", "(sus)", ("teh", 5, "en_US"))[0] == "the"
                    expect_error("AddWord", "(sdbs)", ("zzinvalidwordzz", 1.0, False, "en_US"),
                                 "not supported")
                else:
                    assert [word for word, _ in rows] == ["hello", "help"], rows
                    rows = call("QueryLimited", "(asasuusu)", (["h"], [], 0, 0, "en_US", 1000))
                    assert len(rows) == 100, len(rows)
                    assert len(call("Complete", "(sus)", ("h", 1000, "en_US"))) == 100
                    rows = call("QueryLimited", "(asasuusu)", (["hel"], ["o"], 5, 5, "en_US", 5))
                    assert [word for word, _ in rows] == ["hello"], rows
                print("PASS: private D-Bus " + ("pinned English Patricia dictionary" if args.patricia_dict else
                      "unified completion, ranking, bounds, language errors and missing data"))
            finally:
                process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()


if __name__ == "__main__":
    main()
