# TypeScript / JavaScript — TSDoc

**Public/internal signal**: public = `export`ed and reachable from the package entry point (or the app's shared surface). Internal = unexported, in a conventionally-internal directory, or tagged `@internal`. When everything in a codebase is `export`ed for testing, treat "imported outside its own directory" as the public signal.

**Summary mood**: third person present — "Splits a dotted capability ID…", the TSDoc/JSDoc norm.

## Public shape

```ts
/**
 * Splits a dotted capability ID into its three structured segments.
 *
 * The first two dots separate the segments; further dots are preserved
 * inside `name`.
 *
 * @param dotted - A `<owner>.<kind>.<name>` string.
 * @returns The structured form with `owner`, `kindSegment`, and `name`.
 * @throws {@link RangeError}
 * Thrown if `dotted` has fewer than two `.` separators.
 *
 * @example
 * ```ts
 * parseCapabilityId("turbine.lint.placeholder");
 * // => { owner: "turbine", kindSegment: "lint", name: "placeholder" }
 * ```
 */
export function parseCapabilityId(dotted: string): CapabilityId {
```

Dialect specifics on top of the core section rules:

- **`@param`**: hyphen after the name, no type braces in TypeScript — the signature owns the types. In untyped JavaScript the braces carry the type: `@param {string} dotted - …`.
- **`@returns`**: omit for `void` / `Promise<void>`.
- **`@deprecated`**: always with the replacement — `@deprecated Use {@link parseCapabilityId} instead.`
- Optional tags: `@remarks` (maintainer-only rationale), `@see` (tightly-coupled symbols), `@defaultValue` (when the default isn't visible in the signature).
- `{@link Symbol}` and backticked names must resolve in the same codebase — the checkable form of "the rule, not the citation".

## Internal helpers

One line, single-line block, no tags:

```ts
/** Coerce array defaults to frozen copies so shared config objects stay immutable. */
function coerceDefault(value: unknown): unknown {
```

## Classes and interfaces

- **Class**: the class doc states its role; constructor `@param`s go on the constructor (unlike Python — TS tooling reads them there).
- **Interfaces / type aliases**: one line on what the shape represents; per-property one-liners only when name + type don't already say it (a props interface documents each prop this way).
- **Getters/accessors**: treat as an attribute — one line.
- **Overloads**: document each overload signature, not the implementation — editors surface the doc of the matched overload (the inverse of Python's rule).
- **Overrides**: a member that doesn't refine the base contract gets `/** {@inheritDoc Base.method} */` — a full doc only when the override changes the promise.

## Functions in all their forms

- **Arrow functions assigned to `const`**: documented exactly like function declarations.
- **Inline callbacks**: can't carry a doc — promote to a named function if the logic needs explaining.
- **Test cases**: the `it("…")` / `test("…")` title is the docstring — make it state what correct looks like; no comment block needed.

## Module docs

The package entry point gets a `@packageDocumentation` block. Other files get a leading block comment — one sentence naming the file's role — only when the filename and location don't already make it obvious.

```ts
/**
 * Adapter discovering capability extensions through package.json entry points.
 *
 * @packageDocumentation
 */
```

## Linter

`eslint-plugin-tsdoc` validates tag syntax; `eslint-plugin-jsdoc` enforces presence and consistency (`jsdoc/require-jsdoc` with `publicOnly` off matches the presence-never-lapses rule). `@example` blocks aren't executed — keep them honest by hand or mirror them in a test.
