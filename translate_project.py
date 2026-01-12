#!/usr/bin/env python3
"""translate_project.py

Utility script to translate Russian (Cyrillic) text in the project's files to English.
It walks the repository, detects files containing Cyrillic characters, backs them up,
and replaces the Russian text using the Google Translate service via the `googletrans` library.

Requirements:
- python3
- pip install googletrans==4.0.0-rc1

Usage:
    python3 translate_project.py
"""

import os
import re
import shutil
import sys
from pathlib import Path

try:
    from googletrans import Translator
except ImportError:
    print("googletrans library not installed. Install with: pip install googletrans==4.0.0-rc1")
    sys.exit(1)

# Regex to detect Cyrillic characters
CYRILLIC_RE = re.compile(r"[\u0400-\u04FF]")

translator = Translator()

def contains_cyrillic(text: str) -> bool:
    return bool(CYRILLIC_RE.search(text))

def translate_text(text: str) -> str:
    # googletrans may raise errors; we catch them and fallback to original text
    try:
        result = translator.translate(text, src='ru', dest='en')
        return result.text
    except Exception as e:
        print(f"Translation error: {e}. Leaving original text.")
        return text

def process_file(file_path: Path):
    try:
        with file_path.open('r', encoding='utf-8') as f:
            content = f.read()
    except Exception as e:
        print(f"Skipping {file_path}: {e}")
        return

    if not contains_cyrillic(content):
        return  # No Russian text

    # Backup original
    backup_path = file_path.with_suffix(file_path.suffix + ".bak")
    shutil.copy2(file_path, backup_path)
    print(f"Created backup: {backup_path}")

    # Translate line by line to preserve formatting where possible
    new_lines = []
    for line in content.splitlines(keepends=True):
        if contains_cyrillic(line):
            # Translate only the Cyrillic parts of the line
            # Simple approach: translate the whole line
            translated = translate_text(line)
            new_lines.append(translated)
        else:
            new_lines.append(line)

    new_content = "".join(new_lines)
    with file_path.open('w', encoding='utf-8') as f:
        f.write(new_content)
    print(f"Translated {file_path}")

def main():
    repo_root = Path(__file__).resolve().parent.parent  # assuming script is in a subdir
    exclude_dirs = {".git", "translation", "node_modules", "venv", "__pycache__"}
    for root, dirs, files in os.walk(repo_root):
        # modify dirs in-place to skip excluded
        dirs[:] = [d for d in dirs if d not in exclude_dirs]
        for name in files:
            # Skip binary files based on simple extensions
            if name.lower().endswith(('.png', '.jpg', '.jpeg', '.gif', '.pdf', '.zip', '.exe', '.dll')):
                continue
            file_path = Path(root) / name
            process_file(file_path)

if __name__ == "__main__":
    main()
