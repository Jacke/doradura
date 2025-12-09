#!/usr/bin/env python3
"""
Script to refactor main.rs by replacing admin command handlers
"""

def find_command_block(lines, start_pattern, end_pattern):
    """Find the start and end indices of a command block"""
    start_idx = None
    end_idx = None
    brace_count = 0
    found_start = False

    for i, line in enumerate(lines):
        if start_pattern in line and not found_start:
            start_idx = i
            found_start = True
            # Count braces on the start line
            brace_count += line.count('{') - line.count('}')
            continue

        if found_start:
            brace_count += line.count('{') - line.count('}')
            if brace_count == 0 and '}' in line:
                end_idx = i + 1  # Include the closing brace line
                break

    return start_idx, end_idx

def main():
    # Read the file
    with open('src/main.rs', 'r') as f:
        lines = f.readlines()

    print(f"Original file: {len(lines)} lines")

    # Define replacements
    replacements = {
        'Command::Users =>': '''                                Command::Users => {
                                    let username = msg.from.as_ref().and_then(|u| u.username.as_deref());
                                    let _ = handle_users_command(&bot, msg.chat.id, username, db_pool.clone()).await;
                                }
''',
        'Command::Setplan =>': '''                                Command::Setplan => {
                                    let username = msg.from.as_ref().and_then(|u| u.username.as_deref());
                                    let message_text = msg.text().unwrap_or("");
                                    let _ = handle_setplan_command(&bot, msg.chat.id, username, message_text, db_pool.clone()).await;
                                }
''',
        'Command::Admin =>': '''                                Command::Admin => {
                                    let username = msg.from.as_ref().and_then(|u| u.username.as_deref());
                                    let _ = handle_admin_command(&bot, msg.chat.id, username, db_pool.clone()).await;
                                }
''',
    }

    # Process each replacement
    for pattern, replacement in replacements.items():
        start_idx, end_idx = find_command_block(lines, pattern, None)
        if start_idx is not None and end_idx is not None:
            print(f"Found {pattern} at lines {start_idx+1}-{end_idx}")
            print(f"  Replacing {end_idx - start_idx} lines with {len(replacement.splitlines())} lines")
            # Replace the block
            lines[start_idx:end_idx] = [replacement]
        else:
            print(f"WARNING: Could not find {pattern}")

    # Write back
    with open('src/main.rs', 'w') as f:
        f.writelines(lines)

    print(f"\nRefactored file: {len(lines)} lines")
    print(f"Reduced by: {1588 - len(lines)} lines")

if __name__ == '__main__':
    main()
