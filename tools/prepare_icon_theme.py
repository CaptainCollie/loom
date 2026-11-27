#!/usr/bin/env python3
"""Prepare Loom's standalone Material colors from a local pinned source tree.

Python 3.10+, standard library only; no network access or runtime dependency.
Example:
  python tools/prepare_icon_theme.py target/icon-theme-sources/material-icon-theme-95cc434119c669e0ff65a6bccfd026ec70e16b63
"""

import argparse
import json
from pathlib import Path
import re
import shutil
import xml.etree.ElementTree as ET


REVISION = "95cc434119c669e0ff65a6bccfd026ec70e16b63"
SOURCE_URL = "https://github.com/zed-extensions/material-icon-theme"
FALLBACK = "#7E8EA8"


def hex_rgb(paint):
    """Normalize a literal SVG hex paint; discard any alpha channel."""
    if not re.fullmatch(r"#[0-9a-fA-F]{3,4}|#[0-9a-fA-F]{6}(?:[0-9a-fA-F]{2})?", paint):
        return None
    digits = paint[1:]
    if len(digits) in (3, 4):
        digits = "".join(c * 2 for c in digits[:3])
    return "#" + digits[:6].upper()


def primary_color(path, problems):
    """First chromatic fill/stroke in document order, then a usable neutral."""
    try:
        root = ET.parse(path).getroot()
    except (OSError, ET.ParseError) as error:
        problems.append({"reason": "missing_or_invalid_svg", "detail": str(error)})
        return FALLBACK
    colors = []
    unsupported = set()
    for element in root.iter():
        for name, value in element.attrib.items():
            paints = []
            if name in ("fill", "stroke"):
                paints.append(value.strip())
            elif name == "style":
                for declaration in value.split(";"):
                    key, separator, paint = declaration.partition(":")
                    if separator and key.strip() in ("fill", "stroke"):
                        paints.append(paint.strip())
            for paint in paints:
                color = hex_rgb(paint)
                if color:
                    colors.append(color)
                else:
                    unsupported.add(paint)
    for color in colors:
        if len({color[1:3], color[3:5], color[5:7]}) > 1:
            return color
    for color in colors:
        if color not in ("#000000", "#FFFFFF"):
            return color
    problems.append({"reason": "no_usable_literal_paint", "unsupported_paints": sorted(unsupported)})
    return FALLBACK


def prepare(source, output, report_path, license_path):
    source = source.resolve()
    data = json.loads((source / "icon_themes/material-icon-theme.json").read_text(encoding="utf-8"))
    themes = data["themes"]
    if len(themes) != 1 or themes[0]["name"] != "Material Icon Theme":
        raise ValueError("expected the pinned Material Icon Theme source format")
    theme = themes[0]
    issues = []
    cache = {}

    def color_for(relative):
        path = (source / relative).resolve()
        if not path.is_relative_to(source):
            raise ValueError(f"SVG path escapes source tree: {relative}")
        if relative not in cache:
            problems = []
            cache[relative] = primary_color(path, problems)
            issues.extend({"asset": relative, **problem} for problem in problems)
        return cache[relative]

    colors = {key: color_for(icon["path"]) for key, icon in theme["file_icons"].items()}
    referenced = set(theme["file_stems"].values()) | set(theme["file_suffixes"].values())
    missing_types = sorted(referenced - colors.keys())
    for key in missing_types:
        colors[key] = FALLBACK
    directory_colors = {
        name: color_for(icon["collapsed"])
        for name, icon in theme["named_directory_icons"].items()
    }
    result = {
        "version": 1,
        "name": "Material",
        "file_stems": theme["file_stems"],
        "file_suffixes": theme["file_suffixes"],
        "colors": colors,
        "directory_colors": directory_colors,
        "default_file_color": FALLBACK,
        "default_directory_color": color_for(theme["directory_icons"]["collapsed"]),
    }
    fallback_assets = {issue["asset"] for issue in issues}
    fallback_types = set(missing_types) | {
        key for key, icon in theme["file_icons"].items()
        if icon["path"] in fallback_assets
    }
    report = {
        "source": SOURCE_URL,
        "revision": REVISION,
        "source_archive": f"https://codeload.github.com/zed-extensions/material-icon-theme/zip/{REVISION}",
        "source_theme": f"{SOURCE_URL}/blob/{REVISION}/icon_themes/material-icon-theme.json",
        "license": "Apache-2.0",
        "heuristic": "First literal chromatic fill/stroke in SVG document order, including inline styles; otherwise first neutral other than pure white/black; otherwise #7E8EA8. Hex RGB/RGBA supported; alpha ignored.",
        "counts": {key: len(result[key]) for key in ("file_stems", "file_suffixes", "colors", "directory_colors")},
        "unique_svg_assets_read": len(cache),
        "missing_file_icon_types": missing_types,
        "asset_fallbacks": issues,
        "fallback_type_associations": {
            key: {
                field: sum(value == key for value in theme[field].values())
                for field in ("file_stems", "file_suffixes")
            }
            for key in sorted(fallback_types)
        },
        "fallback_color": FALLBACK,
    }
    for path, value in ((output, result), (report_path, report)):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(value, indent=2, sort_keys=True, ensure_ascii=False) + "\n", encoding="utf-8")
    license_path.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(source / "LICENSE", license_path)
    print(json.dumps(report, indent=2))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path, help="local extracted pinned Material repository")
    parser.add_argument("--output", type=Path, default=Path("assets/icon-themes/material.json"))
    parser.add_argument("--report", type=Path, default=Path("assets/icon-themes/material-source-report.json"))
    parser.add_argument("--license", type=Path, default=Path("assets/icon-themes/material-LICENSE"))
    args = parser.parse_args()
    prepare(args.source, args.output, args.report, args.license)


if __name__ == "__main__":
    main()
