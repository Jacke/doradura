#!/usr/bin/env python3
"""Remove duplicate functions from main.rs"""

def find_function_end(lines, start_idx):
    """Find the end of a function by tracking braces"""
    brace_count = 0
    in_function = False

    for i in range(start_idx, len(lines)):
        line = lines[i]

        if '{' in line:
            brace_count += line.count('{')
            in_function = True

        if '}' in line:
            brace_count -= line.count('}')

        if in_function and brace_count == 0:
            return i + 1  # Include the closing brace line

    return len(lines)

def main():
    with open('src/main.rs', 'r') as f:
        lines = f.readlines()

    print(f"Original: {len(lines)} lines")

    # Functions to remove (with line numbers from grep)
    funcs_to_remove = [
        ('fn is_message_addressed_to_bot', 633),
        ('async fn exponential_backoff', 682),
        ('const VOICE_FILES', 690),
        ('fn convert_wav_to_ogg_opus', 706),
        ('async fn send_voice_with_waveform', 764),
    ]

    # Mark lines for deletion
    lines_to_delete = set()

    for func_name, approx_line in funcs_to_remove:
        # Find exact line
        for i, line in enumerate(lines):
            if i >= approx_line - 10 and i <= approx_line + 10:  # Search within range
                if func_name in line:
                    print(f"Found {func_name} at line {i+1}")
                    end_idx = find_function_end(lines, i)
                    print(f"  Removing lines {i+1} to {end_idx}")
                    for j in range(i, end_idx):
                        lines_to_delete.add(j)
                    break

    # Create new list without deleted lines
    new_lines = [line for i, line in enumerate(lines) if i not in lines_to_delete]

    with open('src/main.rs', 'w') as f:
        f.writelines(new_lines)

    print(f"\nNew file: {len(new_lines)} lines")
    print(f"Removed: {len(lines) - len(new_lines)} lines")

if __name__ == '__main__':
    main()
