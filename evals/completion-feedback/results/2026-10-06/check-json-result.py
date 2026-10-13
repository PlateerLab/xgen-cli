#!/usr/bin/env python3
"""Host-side JSON comparison with bounded, factual repair diagnostics."""
import argparse
import json
from pathlib import Path
import sys

MAX_INPUT_BYTES = 1_048_576
MAX_DIAGNOSTIC_BYTES = 4096


def canonical(value):
    # Preserve JSON types and number representations; ignore object key order.
    return json.dumps(value, sort_keys=True, ensure_ascii=True, allow_nan=False,
                      separators=(',', ':'))


def read_json(path):
    def unique(pairs):
        result = {}
        for key, value in pairs:
            if key in result:
                raise ValueError('duplicate_key')
            result[key] = value
        return result

    with Path(path).open('rb') as stream:
        raw = stream.read(MAX_INPUT_BYTES + 1)
    if len(raw) > MAX_INPUT_BYTES:
        raise ValueError('input_too_large')
    return json.loads(raw, object_pairs_hook=unique,
                      parse_constant=lambda _: (_ for _ in ()).throw(ValueError('nonfinite_number')))


def emit(reason, *, expected=None, actual=None, feedback=True):
    if feedback:
        record = {'format_version': 1, 'status': 'failed', 'reason': reason}
        if reason == 'value_mismatch':
            record.update(expected=expected, actual=actual)
        encoded = canonical(record)
        if len(encoded.encode('ascii')) + 1 > MAX_DIAGNOSTIC_BYTES:
            record.pop('expected', None)
            record.pop('actual', None)
            record['values_omitted'] = True
            encoded = canonical(record)
        print(encoded, file=sys.stderr)
    return 1


def verify(expected, actual_path, *, feedback=True):
    """Expected values come from the trusted checker, never from the candidate."""
    try:
        expected_encoded = canonical(expected)
    except (ValueError, TypeError, RecursionError):
        return emit('invalid_reference', feedback=feedback)
    try:
        actual = read_json(actual_path)
        actual_encoded = canonical(actual)
    except FileNotFoundError:
        return emit('missing_result', feedback=feedback)
    except (ValueError, UnicodeError, RecursionError):
        return emit('invalid_result_json', feedback=feedback)
    except OSError:
        return emit('unreadable_result', feedback=feedback)
    if actual_encoded == expected_encoded:
        return 0
    return emit('value_mismatch', expected=expected, actual=actual, feedback=feedback)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--expected', type=Path, required=True)
    parser.add_argument('--actual', type=Path, required=True)
    args = parser.parse_args()
    try:
        expected = read_json(args.expected)
    except (OSError, ValueError, UnicodeError, RecursionError):
        return emit('invalid_reference')
    return verify(expected, args.actual)


if __name__ == '__main__':
    raise SystemExit(main())
