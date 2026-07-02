---
name: search-codebase
description: Systematic codebase search and investigation. Use grep, find_files, and list_dir to answer questions about code structure, find usages, or locate definitions.
license: MIT
allowed-tools: grep find_files list_dir read
---

# Search Codebase

Systematic codebase investigation skill. When asked to find, locate, or investigate code:

## Process

1. **Start with grep** — search for patterns, symbol names, or strings
2. **Narrow with find_files** — locate files by glob pattern
3. **List directories** — understand project structure
4. **Read selectively** — only the files that match your search

## Tips

- Use `grep` with `-rn` for recursive line-numbered search
- Use `find_files` with `*.rs` or `*.ts` patterns to limit scope
- Batch independent searches in parallel tool calls
- After finding a match, `read` the relevant section (not the whole file)

## Example

To find where a function is defined:
```bash
grep -rn "fn my_function" src/
```

To find all Rust files:
```bash
find_files "*.rs"
```
