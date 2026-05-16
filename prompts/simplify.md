## Code Simplification Mode

You are in **code simplification mode**. Simplify and refine code for clarity, consistency, and maintainability while preserving exact functionality. Focus on recently modified code unless instructed otherwise.

**Announce at start:** "I'm using the simplify prompt. I will refine the code for clarity without changing behavior."

## Guidelines

1. **Preserve functionality** — never change what the code does, only how it does it.
2. **Apply project standards** — follow established conventions from AGENTS.md, CLAUDE.md (import style, naming, types, error handling).
3. **Enhance clarity** — reduce nesting, eliminate redundancy, consolidate related logic, remove comments describing obvious code. Prefer explicit code over compact code.
4. **Maintain balance** — avoid over-simplification. Do not create clever solutions that are hard to understand. Do not combine too many concerns. Do not prioritize fewer lines over readability.
5. **Focus scope** — only refine recently modified code unless instructed otherwise.

## Process

1. Read the code to be simplified.
2. Check for related code that needs matching changes (grep, glob).
3. Apply simplifications one conceptual change at a time.
4. Run relevant tests after each change.
5. Run the full test suite and linters after all changes.
6. Present key changes to the user with brief reasons.

## What to Simplify

- Deeply nested conditionals that can be flattened.
- Duplicated logic that can be consolidated.
- Overly complex expressions that can be broken down.
- Functions that do too much and can be split.
- Dense one-liners that sacrifice readability.
- Unused parameters, variables, or imports.

## What NOT to Change

- Public API or interface signatures.
- Behavior, output format, or error types.
- Performance characteristics (do not make O(n) into O(n^2)).
- Comments documenting non-obvious design decisions or workarounds.
- Existing test coverage.
