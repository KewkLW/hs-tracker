#!/usr/bin/env python3
"""Generate the Rust lookup for named-item roll ranges.

The helper's public /items page does not expose this dataset as a stable JSON
endpoint.  It ships the raw datamined item array in one of the page's hashed
JavaScript bundles instead.  This script discovers those bundles from the page,
finds the first ``JSON.parse`` payload whose JSON begins with
``[[{"chaseDropRate"``, and reads the item records out of that payload.

Only genuinely rolled values are emitted: two-number ``[min, max]`` entries in
``itemBaseStatStruct`` where ``min < max``. Fixed scalars and booleans are not
quality rolls and deliberately stay out of the table. Neither are categorical
ranges which choose a class or spell: a larger internal ID is not a better
item roll.
"""

from __future__ import annotations

import argparse
import json
import math
import re
import sys
from concurrent.futures import ThreadPoolExecutor, as_completed
from dataclasses import dataclass
from html.parser import HTMLParser
from pathlib import Path
from typing import Any, Iterator
from urllib.parse import urljoin, urlparse
from urllib.request import Request, urlopen


ROOT = Path(__file__).resolve().parent.parent
DEFAULT_PAGE = "https://hero-siege-helper.vercel.app/items"
DEFAULT_OUTPUT = ROOT / "src-tauri" / "src" / "item_rolls.rs"
JSON_START = '[[{"chaseDropRate"'
PARSE_START = re.compile(r"JSON\.parse\(\s*(['\"])(?P<payload>\[\[\{\"chaseDropRate\")")
USER_AGENT = "HS-Tracker item-roll generator/1.0"

# These top-level fields do arrive as numbers, so custom exact-value alerts may
# legitimately use them. Their ranges select an identity, though, rather than
# a quality: class for 21, random granted spell for the others. Treating 433 as
# a better spell than 2 made otherwise excellent items fail or pass according
# to an arbitrary catalog number.
NON_QUALITY_STAT_IDS = {21, 202, 205, 208, 419}


class BundleLinks(HTMLParser):
    """Collect JavaScript assets in the order the /items document names them."""

    def __init__(self) -> None:
        super().__init__(convert_charrefs=True)
        self.links: list[str] = []

    def handle_starttag(self, _tag: str, attrs: list[tuple[str, str | None]]) -> None:
        for name, value in attrs:
            if name not in {"src", "href"} or not value:
                continue
            if urlparse(value).path.endswith(".js"):
                self.links.append(value)


def fetch_text(url: str, timeout: float) -> tuple[str, str]:
    request = Request(url, headers={"User-Agent": USER_AGENT})
    with urlopen(request, timeout=timeout) as response:
        body = response.read()
        final_url = response.geturl()
        charset = response.headers.get_content_charset() or "utf-8"
    return body.decode(charset), final_url


def discover_bundles(page_url: str, timeout: float) -> list[str]:
    html, final_url = fetch_text(page_url, timeout)
    parser = BundleLinks()
    parser.feed(html)

    # A preload and a script tag can name the same chunk.  Keep document order,
    # because "first payload" must not depend on hash-table ordering.
    seen: set[str] = set()
    bundles: list[str] = []
    for link in parser.links:
        absolute = urljoin(final_url, link)
        if absolute not in seen:
            seen.add(absolute)
            bundles.append(absolute)
    if not bundles:
        raise RuntimeError(f"no JavaScript bundles discovered from {final_url}")
    return bundles


def js_string_end(source: str, start: int, quote: str) -> int:
    """Return the closing quote for a JavaScript string starting at *start*."""

    cursor = start
    while cursor < len(source):
        char = source[cursor]
        if char == "\\":
            # Every escape consumes at least the following codepoint.  This is
            # enough to find the delimiter even for \uXXXX and \u{...}: their
            # digits cannot themselves close the string.
            cursor += 2
            continue
        if char == quote:
            return cursor
        cursor += 1
    raise ValueError("unterminated JSON.parse JavaScript string")


def _hex_escape(raw: str, start: int, count: int) -> tuple[str, int]:
    digits = raw[start : start + count]
    if len(digits) != count or any(c not in "0123456789abcdefABCDEF" for c in digits):
        raise ValueError(f"invalid hexadecimal JavaScript escape near offset {start}")
    return chr(int(digits, 16)), start + count


def decode_js_string(raw: str) -> str:
    """Decode the escapes used by a quoted JavaScript string without eval()."""

    simple = {
        "'": "'",
        '"': '"',
        "\\": "\\",
        "/": "/",
        "b": "\b",
        "f": "\f",
        "n": "\n",
        "r": "\r",
        "t": "\t",
        "v": "\v",
        "0": "\0",
    }
    out: list[str] = []
    cursor = 0
    while cursor < len(raw):
        char = raw[cursor]
        if char != "\\":
            out.append(char)
            cursor += 1
            continue

        cursor += 1
        if cursor >= len(raw):
            raise ValueError("trailing backslash in JavaScript string")
        escape = raw[cursor]
        cursor += 1
        if escape in simple:
            out.append(simple[escape])
        elif escape == "x":
            value, cursor = _hex_escape(raw, cursor, 2)
            out.append(value)
        elif escape == "u":
            if cursor < len(raw) and raw[cursor] == "{":
                end = raw.find("}", cursor + 1)
                if end < 0:
                    raise ValueError("unterminated JavaScript Unicode escape")
                digits = raw[cursor + 1 : end]
                if not digits or any(c not in "0123456789abcdefABCDEF" for c in digits):
                    raise ValueError("invalid JavaScript Unicode escape")
                codepoint = int(digits, 16)
                if codepoint > 0x10FFFF:
                    raise ValueError("JavaScript Unicode escape is outside Unicode")
                out.append(chr(codepoint))
                cursor = end + 1
            else:
                value, cursor = _hex_escape(raw, cursor, 4)
                out.append(value)
        elif escape == "\r":
            # JavaScript line continuation; CRLF is one continuation.
            if cursor < len(raw) and raw[cursor] == "\n":
                cursor += 1
        elif escape == "\n":
            pass
        else:
            # JavaScript's non-escape character production drops the slash.
            # Keeping that rule here is safer than Python's unicode_escape,
            # which also corrupts ordinary non-ASCII text in the payload.
            out.append(escape)

    # Join UTF-16 surrogate escape pairs without touching ordinary Unicode.
    joined = "".join(out)
    combined: list[str] = []
    cursor = 0
    while cursor < len(joined):
        high = ord(joined[cursor])
        if 0xD800 <= high <= 0xDBFF and cursor + 1 < len(joined):
            low = ord(joined[cursor + 1])
            if 0xDC00 <= low <= 0xDFFF:
                combined.append(chr(0x10000 + ((high - 0xD800) << 10) + low - 0xDC00))
                cursor += 2
                continue
        combined.append(joined[cursor])
        cursor += 1
    return "".join(combined)


def extract_item_array(bundle: str) -> Any | None:
    """Extract the first raw item-array JSON.parse payload in one bundle."""

    for match in PARSE_START.finditer(bundle):
        quote = match.group(1)
        start = match.start("payload")
        end = js_string_end(bundle, start, quote)
        # Reject a coincidental marker inside a string passed somewhere other
        # than JSON.parse.  Minified output normally has ')' immediately here;
        # whitespace is harmless.
        if not bundle[end + 1 :].lstrip().startswith(")"):
            continue
        decoded = decode_js_string(bundle[start:end])
        if not decoded.startswith(JSON_START):
            continue
        try:
            payload = json.loads(decoded)
        except json.JSONDecodeError as error:
            raise ValueError(f"item JSON.parse payload is not valid JSON: {error}") from error
        if not isinstance(payload, list):
            raise ValueError("item JSON.parse payload is not an array")
        return payload
    return None


def fetch_item_array(page_url: str, timeout: float, workers: int) -> tuple[Any, str, int]:
    bundles = discover_bundles(page_url, timeout)
    bodies: dict[str, str] = {}
    errors: dict[str, Exception] = {}
    with ThreadPoolExecutor(max_workers=max(1, workers)) as pool:
        futures = {pool.submit(fetch_text, url, timeout): url for url in bundles}
        for future in as_completed(futures):
            url = futures[future]
            try:
                bodies[url] = future.result()[0]
            except Exception as error:  # report all failures if discovery fails
                errors[url] = error

    for url in bundles:
        body = bodies.get(url)
        if body is None:
            continue
        payload = extract_item_array(body)
        if payload is not None:
            return payload, url, len(bundles)

    detail = "; ".join(f"{url}: {error}" for url, error in errors.items())
    if detail:
        detail = f" ({detail})"
    raise RuntimeError(f"no JSON.parse payload beginning {JSON_START!r} in {len(bundles)} bundles{detail}")


def is_number(value: Any) -> bool:
    return isinstance(value, (int, float)) and not isinstance(value, bool) and math.isfinite(float(value))


def integer(value: Any, field: str, maximum: int) -> int:
    if not is_number(value) or not float(value).is_integer():
        raise ValueError(f"named item has invalid {field}: {value!r}")
    result = int(value)
    if not 0 <= result <= maximum:
        raise ValueError(f"named item has out-of-range {field}: {result}")
    return result


def stat_id(value: Any) -> int | None:
    if not isinstance(value, str) or not value.isdecimal():
        return None
    result = int(value)
    return result if 0 <= result <= 0xFFFF else None


def named_records(value: Any) -> Iterator[dict[str, Any]]:
    """Walk arbitrary nesting and yield records marked as named by the game."""

    if isinstance(value, dict):
        definition = value.get("itemBaseDefinitionStruct")
        if isinstance(definition, dict):
            named = definition.get("c")
            if named is True or (is_number(named) and float(named) == 1.0):
                yield value
        for child in value.values():
            yield from named_records(child)
    elif isinstance(value, list):
        for child in value:
            yield from named_records(child)


@dataclass(frozen=True, order=True)
class RollRange:
    id: int
    minimum: float
    maximum: float


@dataclass(frozen=True)
class Generated:
    records: int
    items: dict[int, tuple[RollRange, ...]]
    conditional_items: int

    @property
    def ranges(self) -> int:
        return sum(len(item) for item in self.items.values())


def has_variable_range(value: Any) -> bool:
    """Whether a secondary/conditional stat tree contains a real roll."""

    if isinstance(value, dict):
        return any(has_variable_range(child) for child in value.values())
    if isinstance(value, list):
        if len(value) == 2 and all(is_number(part) for part in value):
            return float(value[0]) < float(value[1])
        return any(has_variable_range(child) for child in value)
    return False


def collect_ranges(payload: Any) -> Generated:
    records = 0
    conditional_items = 0
    items: dict[int, tuple[RollRange, ...]] = {}
    for record in named_records(payload):
        records += 1
        definition = record["itemBaseDefinitionStruct"]
        item_type = integer(record.get("itemType"), "itemType", 0xFF)
        item_id = integer(definition.get("b"), "itemBaseDefinitionStruct.b", 0xFFFF)
        weapon_type = integer(definition.get("j", 0), "itemBaseDefinitionStruct.j", 0xFF)
        key = (item_type << 24) | (item_id << 8) | weapon_type

        # A small number of items roll one branch out of a damage-type table.
        # Flattening only the ordinary base table would call those items
        # "all high" while silently ignoring the selected branch. Until the
        # branch selector is carried through the packet parser, omit the whole
        # identity so scoring fails closed rather than producing a false alert.
        if has_variable_range(record.get("itemBaseDamageTypeStatStruct")):
            conditional_items += 1
            continue

        found: list[RollRange] = []
        stats = record.get("itemBaseStatStruct")
        if isinstance(stats, dict):
            for raw_id, raw_range in stats.items():
                sid = stat_id(raw_id)
                if sid is None or sid in NON_QUALITY_STAT_IDS or not isinstance(raw_range, list) or len(raw_range) != 2:
                    continue
                minimum, maximum = raw_range
                if not is_number(minimum) or not is_number(maximum):
                    continue
                minimum = float(minimum)
                maximum = float(maximum)
                if minimum < maximum:
                    found.append(RollRange(sid, minimum, maximum))
        ranges = tuple(sorted(found))
        if not ranges:
            continue
        old = items.get(key)
        if old is not None and old != ranges:
            raise ValueError(f"named-item identity 0x{key:08x} has conflicting roll ranges")
        items[key] = ranges

    if not records:
        raise ValueError("item payload contains no named records")
    if not items:
        raise ValueError("named records contain no variable roll ranges")
    return Generated(records, items, conditional_items)


def rust_float(value: float) -> str:
    rendered = repr(value)
    # Rust accepts scientific notation, but an integral-looking literal needs a
    # decimal point to infer f64 in a struct field consistently.
    if "e" not in rendered.lower() and "." not in rendered:
        rendered += ".0"
    return rendered


def render_rust(generated: Generated) -> str:
    flat: list[RollRange] = []
    index: list[tuple[int, int, int]] = []
    for key, ranges in sorted(generated.items.items()):
        start = len(flat)
        flat.extend(ranges)
        if len(ranges) > 0xFFFF:
            raise ValueError(f"identity 0x{key:08x} has too many stat ranges")
        index.append((key, start, len(ranges)))

    lines = [
        "// Generated by tools/gen_item_rolls.py — do not edit by hand.",
        "// Source: the first raw item JSON.parse array discovered from hero-siege-helper /items.",
        f"// {generated.records} named records; {len(index)} items with {len(flat)} variable roll ranges.",
        f"// {generated.conditional_items} conditional damage-type item(s) omitted to fail closed.",
        "",
        "#[derive(Clone, Copy, Debug)]",
        "pub struct StatRange {",
        "    pub id: u16,",
        "    pub min: f64,",
        "    pub max: f64,",
        "}",
        "",
        "#[rustfmt::skip]",
        f"static STAT_RANGES: [StatRange; {len(flat)}] = [",
    ]
    lines.extend(
        f"    StatRange {{ id: {roll.id}, min: {rust_float(roll.minimum)}, max: {rust_float(roll.maximum)} }},"
        for roll in flat
    )
    lines += [
        "];",
        "",
        "// packed item identity -> (first range, number of ranges)",
        "#[rustfmt::skip]",
        f"static ITEMS: [(u32, u32, u16); {len(index)}] = [",
    ]
    lines.extend(f"    ({key}, {start}, {length})," for key, start, length in index)
    lines += [
        "];",
        "",
        "fn packed(item_type: i64, id: i64, weapon_type: i64) -> Option<u32> {",
        "    if !(0..=u8::MAX as i64).contains(&item_type)",
        "        || !(0..=u16::MAX as i64).contains(&id)",
        "        || !(0..=u8::MAX as i64).contains(&weapon_type)",
        "    {",
        "        return None;",
        "    }",
        "    Some(((item_type as u32) << 24) | ((id as u32) << 8) | weapon_type as u32)",
        "}",
        "",
        "fn lookup(key: u32) -> Option<&'static [StatRange]> {",
        "    let index = ITEMS",
        "        .binary_search_by_key(&key, |(candidate, _, _)| *candidate)",
        "        .ok()?;",
        "    let (_, start, len) = ITEMS[index];",
        "    let start = start as usize;",
        "    Some(&STAT_RANGES[start..start + len as usize])",
        "}",
        "",
        "/// Variable stat ranges for an exact named-item identity.",
        "///",
        "/// Weapons are keyed by subtype. If that exact identity is absent, the",
        "/// weapon-type-agnostic identity is tried, matching the item-name table.",
        "pub fn ranges(item_type: i64, id: i64, weapon_type: i64) -> Option<&'static [StatRange]> {",
        "    let exact = packed(item_type, id, weapon_type)?;",
        "    if let Some(found) = lookup(exact) {",
        "        return Some(found);",
        "    }",
        "    if weapon_type == 0 {",
        "        return None;",
        "    }",
        "    lookup(packed(item_type, id, 0)?)",
        "}",
        "",
        "#[cfg(test)]",
        "mod tests {",
        "    use super::{ranges, STAT_RANGES};",
        "",
        "    #[test]",
        "    fn invalid_identities_are_safe() {",
        "        for identity in [",
        "            (-1, 0, 0),",
        "            (256, 0, 0),",
        "            (0, -1, 0),",
        "            (0, 65_536, 0),",
        "            (0, 0, -1),",
        "            (0, 0, 256),",
        "        ] {",
        "            assert!(ranges(identity.0, identity.1, identity.2).is_none());",
        "        }",
        "    }",
        "",
        "    #[test]",
        "    fn conditional_damage_type_rolls_fail_closed() {",
        "        // Wraith's Cloak chooses one of five secondary stat branches.",
        "        assert!(ranges(1, 98, 0).is_none());",
        "    }",
        "",
        "    #[test]",
        "    fn categorical_ids_are_not_scored_as_roll_quality() {",
        "        for id in [21, 202, 205, 208, 419] {",
        "            assert!(!STAT_RANGES.iter().any(|range| range.id == id));",
        "        }",
        "    }",
        "}",
        "",
    ]
    return "\n".join(lines)


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--page", default=DEFAULT_PAGE, help=f"items page (default: {DEFAULT_PAGE})")
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT, help=f"Rust output (default: {DEFAULT_OUTPUT})")
    parser.add_argument("--timeout", type=float, default=30.0, help="network timeout per request in seconds")
    parser.add_argument("--workers", type=int, default=8, help="number of bundle downloads in flight")
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(sys.argv[1:] if argv is None else argv)
    payload, bundle, bundle_count = fetch_item_array(args.page, args.timeout, args.workers)
    generated = collect_ranges(payload)
    output = args.output.resolve()
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(render_rust(generated), encoding="utf-8", newline="\n")
    print(f"source: {bundle} ({bundle_count} bundles discovered)")
    print(
        f"named records: {generated.records}; items with ranges: {len(generated.items)}; "
        f"variable ranges: {generated.ranges}; conditional items omitted: "
        f"{generated.conditional_items} -> {output}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
