#### Logic Errors
- Incorrect conditionals, missing `elif` branches, or redundant checks
- Off-by-one errors in ranges and slices
- Variables used before assignment or in an undefined state
- Incorrect loop termination conditions

#### Mutable Default Arguments
- Mutable default arguments (`def f(x=[])`) are shared across calls — use `None` + conditional initialisation instead
- Same issue with `{}`, `set()`, `datetime.now()` as defaults

#### Error Handling
- Bare `except:` that catches all exceptions including `SystemExit` / `KeyboardInterrupt`
- Exceptions swallowed without logging or re-raising
- Missing `finally` or context manager (`with`) for resource cleanup
- Overly broad exception types (`except Exception`) when specific types would be safer

#### Type Safety
- Missing type annotations on function signatures (public APIs should be fully typed)
- `Any` used where a `TypeVar` or `Protocol` would be more precise
- Incorrect use of `is` vs `==` (identity vs equality)

#### Performance
- Unnecessary list comprehensions where generator expressions would suffice
- Repeated member access in loops — hoist to a local variable
- String concatenation in loops (use `''.join(...)`)
- `pandas` operations that vectorise poorly — prefer built-in vectorised methods

#### Security
- SQL queries built with string interpolation instead of parameterised queries
- `eval()` / `exec()` or `pickle.loads()` on untrusted data
- Hardcoded secrets, API keys, or connection strings
- `os.system()` or `subprocess(shell=True)` with unsanitised input
