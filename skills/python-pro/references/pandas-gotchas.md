# pandas — gotchas and house style

What the model gets wrong or picks inconsistently without this file. Everything else (merge semantics, groupby mechanics, indexing) it already knows.

## Gotchas

- **Empty strings are not NaN.** `isna()` misses `''`, `'N/A'`, `'null'`, `'-'`. Normalize at the boundary: `pd.read_csv(..., na_values=['', 'N/A', 'null', '-'])` or `df.replace(['', 'N/A'], np.nan)` before any missing-value logic.
- **Nullable dtypes are capital-letter** (`Int64`, `Float64`, `boolean`, `string`) and they're the only ints that hold NA. Safe numeric conversion is the two-step: `pd.to_numeric(s, errors='coerce').astype('Int64')`. (`errors='ignore'` is removed — never suggest it.)
- **`.append()` is removed** (pandas 2.0). `pd.concat([df1, df2], ignore_index=True)` — and never in a loop; collect then concat once.
- **Chained indexing** (`df['a']['b']`, filter-then-assign) silently writes to a copy. Assign through a single `.loc[mask, col]`; take `.copy()` explicitly when a subset will live on. Copy-on-write (default in pandas 3) makes chained assignment *always* a no-op, not just sometimes.
- **`to_datetime` without `format=`** is slow and guesses. Pass `format='%Y-%m-%d'` (or `format='mixed'` deliberately) plus `errors='coerce'`.
- **Categorical groupby produces rows for unobserved categories** unless `observed=True`. Pass it whenever grouping on a categorical.
- **Multi-condition column derivation**: `np.select(conditions, choices, default=...)` — not nested `np.where`, not `.apply(row_func, axis=1)`.

## House style (hot opinions)

- **Method chaining is the unit of pandas work**: one parenthesized chain per transformation — `.rename → .dropna → .assign → .astype → .drop_duplicates → .reset_index(drop=True)` — not a sequence of `df = df[...]` reassignments.
- **Aggregate once**: a single `.groupby(...).agg(named_tuples)` call, never repeated groupbys on the same keys, never `.apply(lambda x: x.mean())` where a built-in exists.
- **Dtypes are part of the schema**: set them at read time (`dtype=` in `read_csv`), use `category` for low-cardinality strings, downcast numerics when memory matters (`memory_usage(deep=True)` to check).
- **Parquet over CSV** for anything that round-trips; CSV is an interchange format, not storage.
- **Chunk what you can't hold**: `read_csv(chunksize=...)`, filter/aggregate per chunk, concat the reductions.
- **pandera at data boundaries** in production — schema validation where data enters, not assertions scattered through transforms.
- **If you must iterate, `itertuples()`** — `iterrows()` is never the answer.
