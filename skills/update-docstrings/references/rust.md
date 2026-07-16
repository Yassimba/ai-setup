# Rust — rustdoc

**Public/internal signal**: public = `pub` and reachable from the crate root — exactly what `cargo doc` renders. Internal = `pub(crate)`, `pub(super)`, or private. Internal items still get a one-line `///`.

**Summary mood**: third person present — "Splits a dotted capability ID…". The first line stands alone: item indexes render only it.

## Public shape

```rust
/// Splits a dotted capability ID into its three structured segments.
///
/// The first two dots separate the segments; further dots are preserved
/// inside `name`.
///
/// # Errors
///
/// Returns [`ParseError::TooFewSegments`] if `dotted` has fewer than two
/// `.` separators.
///
/// # Examples
///
/// ```
/// use caps::parse_capability_id;
///
/// let id = parse_capability_id("turbine.lint.placeholder")?;
/// assert_eq!(id.owner.as_str(), "turbine");
/// # Ok::<(), caps::ParseError>(())
/// ```
pub fn parse_capability_id(dotted: &str) -> Result<CapabilityId, ParseError> {
```

Dialect specifics on top of the core section rules:

- **No parameters section** — rustdoc has none; weave parameter meaning into the prose. A signature whose parameters need a table probably needs a builder or a struct argument instead.
- **`# Errors`**: on every `pub fn` returning `Result` — name each variant and when it fires.
- **`# Panics`**: on every deliberate panic path (asserts, `unwrap` on documented invariants).
- **`# Safety`**: mandatory on every `unsafe fn` — the exact contract the caller must uphold.
- **`# Examples`**: doctests — compiled and run by `cargo test`. Hide setup lines with a leading `# ` so the rendered example stays minimal.
- **Intra-doc links**: [`CapabilityId`], [`ParseError::TooFewSegments`] — they resolve at doc build, the checkable form of "the rule, not the citation".
- **Deprecation**: `#[deprecated(since = "1.2.0", note = "use `parse_capability_id` instead")]` — the attribute, not prose.

## Internal items

One line, no sections:

```rust
/// Coerces list defaults to owned vecs so cached configs never alias the parser's buffer.
fn coerce_default(value: &Value) -> Value {
```

## Types and traits

- **Structs / enums**: the type doc states its role; per-field `///` only when name + type don't already say it. Every enum variant gets a line — variants are API.
- **Traits**: the trait doc states what implementers promise; each method doc states what implementations must do. `# Examples` on the trait showing a typical impl beats repeating one per method.
- **Trait impls**: usually undocumented — the trait's docs cover the contract; add a line only when this impl has surprising behaviour.

## Module and crate docs

`//!` at the top of the file (or `lib.rs` for the crate): one sentence naming the module's role in the system, mechanics after only if the path doesn't make them obvious.

```rust
//! Adapter discovering capability extensions through cargo metadata.
//!
//! Loading a manifest parses its `[package.metadata.capabilities]` table and
//! registers every entry, so each manifest pairs with exactly its own
//! contributions.
```

## Linter

`#![warn(missing_docs)]` in `lib.rs` enforces presence on public items; clippy's `missing_errors_doc`, `missing_panics_doc`, and `missing_safety_doc` enforce the sections. `cargo test --doc` runs the examples.
