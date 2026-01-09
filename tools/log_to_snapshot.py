#!/usr/bin/env python3
"""
Утилита для конвертации логов Telegram бота в snapshot файлы

Использование:
    # Из логов приложения
    ./tools/log_to_snapshot.py --input bot.log --output tests/snapshots/my_test.json

    # Из вывода cargo run
    cargo run 2>&1 | ./tools/log_to_snapshot.py --stdin --output my_snapshot.json

    # Интерактивный режим
    ./tools/log_to_snapshot.py --interactive

Формат логов:
    [DEBUG] Request to https://api.telegram.org/bot.../sendMessage
    Body: {"chat_id":123,"text":"hello"}
    [DEBUG] Response: {"ok":true,"result":{...}}
"""

import argparse
import json
import re
import sys
from datetime import datetime
from typing import List, Dict, Any, Optional, Tuple


class TelegramLogParser:
    """Парсер логов Telegram бота для извлечения API вызовов"""

    # Регулярные выражения для поиска API вызовов
    REQUEST_PATTERN = re.compile(
        r'Request to https://api\.telegram\.org/bot[^/]+(/\w+)',
        re.IGNORECASE
    )
    BODY_PATTERN = re.compile(r'Body:\s*({.+})', re.IGNORECASE | re.DOTALL)
    RESPONSE_PATTERN = re.compile(r'Response:\s*({.+})', re.IGNORECASE | re.DOTALL)

    def __init__(self):
        self.interactions = []
        self.current_request = None
        self.current_body = None

    def parse_file(self, filename: str) -> List[Tuple[Dict, Dict]]:
        """Парсит файл логов и извлекает взаимодействия"""
        with open(filename, 'r', encoding='utf-8') as f:
            return self.parse_lines(f)

    def parse_stdin(self) -> List[Tuple[Dict, Dict]]:
        """Парсит логи из stdin"""
        return self.parse_lines(sys.stdin)

    def parse_lines(self, lines) -> List[Tuple[Dict, Dict]]:
        """Парсит строки логов"""
        for line in lines:
            self._process_line(line)

        return self.interactions

    def _process_line(self, line: str):
        """Обрабатывает одну строку лога"""
        # Проверяем на request
        request_match = self.REQUEST_PATTERN.search(line)
        if request_match:
            self.current_request = {
                'path': request_match.group(1),
                'method': 'POST'  # Большинство API вызовов - POST
            }
            return

        # Проверяем на body
        body_match = self.BODY_PATTERN.search(line)
        if body_match:
            try:
                self.current_body = json.loads(body_match.group(1))
            except json.JSONDecodeError as e:
                print(f"⚠️  Failed to parse request body: {e}", file=sys.stderr)
            return

        # Проверяем на response
        response_match = self.RESPONSE_PATTERN.search(line)
        if response_match and self.current_request:
            try:
                response_json = json.loads(response_match.group(1))

                # Создаем пару request/response
                api_call = {
                    'method': self.current_request['method'],
                    'path': self.current_request['path'],
                    'body': self.current_body or {},
                    'timestamp': int(datetime.now().timestamp())
                }

                api_response = {
                    'status': 200 if response_json.get('ok') else 400,
                    'body': response_json,
                    'headers': {
                        'content-type': 'application/json'
                    }
                }

                self.interactions.append((api_call, api_response))

                # Reset state
                self.current_request = None
                self.current_body = None

            except json.JSONDecodeError as e:
                print(f"⚠️  Failed to parse response: {e}", file=sys.stderr)


def create_snapshot(
    name: str,
    interactions: List[Tuple[Dict, Dict]],
    metadata: Optional[Dict[str, str]] = None
) -> Dict[str, Any]:
    """Создает snapshot из списка взаимодействий"""
    return {
        'name': name,
        'version': '1.0',
        'recorded_at': datetime.utcnow().isoformat() + 'Z',
        'interactions': interactions,
        'metadata': metadata or {}
    }


def interactive_mode():
    """Интерактивный режим для создания snapshot из буфера обмена"""
    print("📝 Интерактивный режим создания snapshot")
    print()

    name = input("Введите имя snapshot (например, 'start_command'): ").strip()
    if not name:
        print("❌ Имя не может быть пустым")
        return

    print("\n📋 Вставьте логи бота (Ctrl+D для завершения):")
    print("=" * 60)

    parser = TelegramLogParser()
    interactions = parser.parse_lines(sys.stdin)

    if not interactions:
        print("\n❌ Не найдено ни одного взаимодействия в логах")
        print("\nЛоги должны содержать строки вида:")
        print("  [DEBUG] Request to https://api.telegram.org/bot.../sendMessage")
        print("  Body: {...}")
        print("  [DEBUG] Response: {...}")
        return

    print(f"\n✅ Найдено взаимодействий: {len(interactions)}")

    # Метаданные
    print("\nДобавьте метаданные (Enter для пропуска):")
    description = input("  Описание: ").strip()
    command = input("  Команда (например, /start): ").strip()

    metadata = {}
    if description:
        metadata['description'] = description
    if command:
        metadata['command'] = command

    # Создаем snapshot
    snapshot = create_snapshot(name, interactions, metadata)

    # Сохраняем
    output_file = f"tests/snapshots/{name}.json"
    with open(output_file, 'w', encoding='utf-8') as f:
        json.dump(snapshot, f, indent=2, ensure_ascii=False)

    print(f"\n💾 Snapshot сохранен: {output_file}")
    print(f"📊 Взаимодействий: {len(interactions)}")


def main():
    parser = argparse.ArgumentParser(
        description='Конвертирует логи Telegram бота в snapshot файлы для тестирования'
    )
    parser.add_argument(
        '--input', '-i',
        help='Файл с логами'
    )
    parser.add_argument(
        '--output', '-o',
        help='Выходной JSON файл (по умолчанию: tests/snapshots/{name}.json)'
    )
    parser.add_argument(
        '--name', '-n',
        help='Имя snapshot (обязательно если не --interactive)'
    )
    parser.add_argument(
        '--stdin',
        action='store_true',
        help='Читать логи из stdin'
    )
    parser.add_argument(
        '--interactive',
        action='store_true',
        help='Интерактивный режим'
    )
    parser.add_argument(
        '--description', '-d',
        help='Описание snapshot'
    )
    parser.add_argument(
        '--command', '-c',
        help='Команда которая тестируется (например, /start)'
    )

    args = parser.parse_args()

    # Интерактивный режим
    if args.interactive:
        interactive_mode()
        return

    # Валидация аргументов
    if not args.name:
        print("❌ Укажите --name для snapshot или используйте --interactive", file=sys.stderr)
        sys.exit(1)

    if not args.input and not args.stdin:
        print("❌ Укажите --input <файл> или --stdin", file=sys.stderr)
        sys.exit(1)

    # Парсинг логов
    log_parser = TelegramLogParser()

    if args.stdin:
        print("📖 Читаем логи из stdin...", file=sys.stderr)
        interactions = log_parser.parse_stdin()
    else:
        print(f"📖 Читаем логи из {args.input}...", file=sys.stderr)
        interactions = log_parser.parse_file(args.input)

    if not interactions:
        print("❌ Не найдено ни одного взаимодействия", file=sys.stderr)
        sys.exit(1)

    print(f"✅ Найдено взаимодействий: {len(interactions)}", file=sys.stderr)

    # Метаданные
    metadata = {}
    if args.description:
        metadata['description'] = args.description
    if args.command:
        metadata['command'] = args.command

    # Создаем snapshot
    snapshot = create_snapshot(args.name, interactions, metadata)

    # Определяем выходной файл
    output_file = args.output or f"tests/snapshots/{args.name}.json"

    # Сохраняем
    with open(output_file, 'w', encoding='utf-8') as f:
        json.dump(snapshot, f, indent=2, ensure_ascii=False)

    print(f"💾 Snapshot сохранен: {output_file}", file=sys.stderr)
    print(f"📊 Взаимодействий: {len(interactions)}", file=sys.stderr)
    print(f"\n📝 Теперь можно использовать в тестах:", file=sys.stderr)
    print(f"   TelegramMock::from_snapshot(\"{args.name}\")", file=sys.stderr)


if __name__ == '__main__':
    main()
