#!/usr/bin/python3
"""Replay deterministic synthetic swipes against a real daemon on a private bus.

These ideal and perturbed key-center paths are integration regressions, not a
measurement of real-world recognition accuracy. No desktop/device is changed.
"""
import argparse
import hashlib
import json
import math
from pathlib import Path
import subprocess
import sys
import tempfile
import time
from gi.repository import Gio, GLib

NAME = 'org.verbisage.Dictionary'
PATH = '/org/verbisage/Dictionary'
IFACE = 'org.verbisage.Dictionary1'
SIGNATURE = '(a(ddu)a(sdddd)us)'
KNOWN_QUALITY_LIMITATIONS = {('swipe', 'jitter')}
WORDS = ['cat', 'dog', 'hello', 'world', 'keyboard', 'typing', 'swipe', 'quick', 'brown', 'test']


def keyboard(scale=1.0, offset=(0.0, 0.0)):
    return [(letter, (col * 40.0 + row * 20.0) * scale + offset[0], row * 50.0 * scale + offset[1],
             36.0 * scale, 46.0 * scale)
            for row, letters in enumerate(['qwertyuiop', 'asdfghjkl', 'zxcvbnm'])
            for col, letter in enumerate(letters)]


def gesture(word, keys, jitter=0.0):
    centers = {label: (x + w / 2.0, y + h / 2.0) for label, x, y, w, h in keys}
    points = []
    for letter in word:
        x, y = centers[letter]
        if not points:
            points.append((x, y, 0))
            continue
        previous_x, previous_y, _ = points[-1]
        if (x, y) == (previous_x, previous_y):
            continue
        for step in range(1, 7):
            points.append((previous_x + (x - previous_x) * step / 6.0,
                           previous_y + (y - previous_y) * step / 6.0, len(points) * 10))
    return [(x + math.sin(index * 1.7) * jitter, y + math.cos(index * 1.3) * jitter, millis)
            for index, (x, y, millis) in enumerate(points)]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--daemon', type=Path, required=True)
    parser.add_argument('--patricia-dict', type=Path, required=True)
    parser.add_argument('--output', type=Path)
    parser.add_argument('--explore', action='store_true', help='record rankings without asserting target rank')
    args = parser.parse_args()
    connection = Gio.bus_get_sync(Gio.BusType.SESSION, None)

    def call(method, signature, values):
        return connection.call_sync(NAME, PATH, IFACE, method, GLib.Variant(signature, values),
                                    None, Gio.DBusCallFlags.NONE, 3000, None).unpack()[0]

    def recognize(points, keys, limit=6, lang='en_US'):
        return call('RecognizeSwipe', SIGNATURE, (points, keys, limit, lang))

    def error(points, keys, fragment, lang='en_US'):
        try:
            recognize(points, keys, lang=lang)
        except GLib.Error as exc:
            assert fragment in str(exc), str(exc)
        else:
            raise AssertionError(f'expected error: {fragment}')

    report = {'corpus': 'deterministic synthetic key-center traces; not human gesture accuracy',
              'dictionary_sha256': hashlib.sha256(args.patricia_dict.read_bytes()).hexdigest(),
              'cases': [], 'errors_checked': []}
    with tempfile.TemporaryDirectory(prefix='verbisage-swipe-test-') as directory:
        directory = Path(directory)
        (directory / 'en_US.dict').symlink_to(args.patricia_dict.resolve())
        config = directory / 'config.toml'
        config.write_text('backend = "patricia"\n[backends.patricia]\ntype = "patricia"\nsystem_dir = "' + str(directory) + '"\n')
        with (directory / 'daemon.log').open('w+') as log:
            daemon = subprocess.Popen([str(args.daemon.resolve()), '--mode', 'dbus', '--config', str(config), '--user-dict', ''],
                                      stdout=log, stderr=log)
            try:
                for _ in range(100):
                    if daemon.poll() is not None:
                        log.seek(0)
                        raise AssertionError(log.read())
                    owner = connection.call_sync('org.freedesktop.DBus', '/org/freedesktop/DBus', 'org.freedesktop.DBus',
                                                 'NameHasOwner', GLib.Variant('(s)', (NAME,)), None,
                                                 Gio.DBusCallFlags.NONE, 1000, None).unpack()[0]
                    if owner:
                        break
                    time.sleep(0.05)
                else:
                    raise AssertionError('daemon did not acquire its bus name')
                assert call('Complete', '(sus)', ('helo', 6, 'en_US'))[0][0] == 'hello'
                for variation in ['ideal', 'jitter', 'scaled-translated']:
                    keys = keyboard(1.75, (100.0, 80.0)) if variation == 'scaled-translated' else keyboard()
                    for word in WORDS:
                        points = gesture(word, keys, 2.0 if variation == 'jitter' else 0.0)
                        started = time.monotonic()
                        candidates = recognize(points, keys)
                        elapsed = (time.monotonic() - started) * 1000.0
                        assert candidates == recognize(points, keys), (word, 'nondeterministic result')
                        assert len(candidates) <= 6 and len({word for word, _ in candidates}) == len(candidates)
                        assert all(math.isfinite(score) and 0.0 <= score <= 1.0 for _, score in candidates)
                        words = [candidate for candidate, _ in candidates]
                        rank = words.index(word) + 1 if word in words else None
                        report['cases'].append({'word': word, 'variation': variation, 'target_rank': rank,
                                                'candidates': candidates, 'milliseconds': round(elapsed, 3),
                                                'trace': points, 'keys': keys})
                        print(word, variation, rank, round(elapsed, 1), words, flush=True)
                        if not args.explore and (word, variation) not in KNOWN_QUALITY_LIMITATIONS:
                            assert rank is not None and rank <= 3, (word, variation, candidates)
                keys = keyboard()
                points = gesture('cat', keys)
                assert recognize(points, keys, 1) == recognize(points, keys, 6)[:1]
                assert recognize(points, keys, 0) == []
                error([], keys, '2..512')
                error([points[0]] * 513, keys, '2..512')
                error([points[0]] * 4, keys, 'no usable motion')
                error([(float('nan'), 0.0, 0)] + points[1:], keys, 'coordinates')
                error(points[:1] + [(points[1][0], points[1][1], 10001)], keys, 'timestamps')
                error(points, [('q', 0.0, 0.0, 0.0, 1.0)] + keys[1:], 'rectangle')
                error(points, [keys[0]] * 26, 'unique')
                error(points, keys, 'invalid language tag', '../invalid')
                error(points, keys, 'no dictionary loaded', 'zz_ZZ')
                assert call('Complete', '(sus)', ('helo', 6, 'en_US'))[0][0] == 'hello'
                report['errors_checked'] = ['empty', 'oversized', 'stationary', 'NaN', 'timestamps',
                                            'invalid rectangle', 'duplicate label', 'invalid language', 'unavailable dictionary']
                report['complete_before_and_after'] = 'helo -> hello passed'
            finally:
                if sys.exc_info()[0] is not None:
                    log.flush()
                    log.seek(0)
                    print(log.read(), file=sys.stderr)
                daemon.terminate()
                try:
                    daemon.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    daemon.kill()
                    daemon.wait()
    report['quality_top3'] = sum(case['target_rank'] is not None and case['target_rank'] <= 3 for case in report['cases'])
    report['quality_cases'] = len(report['cases'])
    report['known_quality_limitations'] = [{'word': word, 'variation': variant} for word, variant in sorted(KNOWN_QUALITY_LIMITATIONS)]
    if args.output:
        args.output.write_text(json.dumps(report, indent=2) + '\n')
    print(f"PASS: real Drift/Patricia replay, bounds and Complete; quality {report['quality_top3']}/{report['quality_cases']} top3, known limitations recorded")


if __name__ == '__main__':
    main()
