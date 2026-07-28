#### Logic Errors
- Incorrect conditionals or missing null/undefined checks
- Off-by-one errors in array bounds or loop conditions
- Async/await issues — missing `await` or awaiting non-promise values
- Incorrect use of `==` vs `===` (type-coercing equality)

#### Type Safety (TypeScript)
- `any` used where `unknown` or a proper type would be safer
- Missing generics on reusable utility types
- Incorrect use of type assertions (as) that bypass type checking
- Function signatures that don't accurately describe possible null/undefined returns

#### React / JSX (when applicable)
- Missing `key` props in list renders
- Expensive computations inside render without `useMemo`
- Effect dependencies incorrect or missing (stale closures)
- State updates that depend on previous state without functional form (`setCount(c => c+1)`)
- Side effects in render function (move to `useEffect`)

#### Security
- `innerHTML`, `dangerouslySetInnerHTML`, or `v-html` without sanitisation
- Unsanitised user input in URL construction or `window.open`
- Hardcoded secrets, tokens, or API keys in client code
- Prototype pollution via unsafe object merge

#### Performance
- Large list renders without virtualisation (react-window / react-virtuoso)
- Unnecessary re-renders — missing `useMemo` / `useCallback` / `React.memo`
- Object spread in hot paths — prefer `Object.assign` or manual assignment
- Bundle size — large libraries imported for one utility function

#### Error Handling
- Unhandled promise rejections (missing `.catch()` or try/catch in async)
- Error boundaries missing or incorrectly placed
- API error responses not checked before accessing data
