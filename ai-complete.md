# Task: complete missing metadata in equations.json

`equations.json` has the shape `{"quantities": [...], "equations": [...]}`.
It is a physics equation library (condensed matter / DFT / many-body theory).
Edit the file in place. It must stay valid JSON in the same schema.

## Fill in these fields

- `equations[].description` - one or two concise sentences: what the equation
  states and its role. Match the tone of existing entries in the file.
- `equations[].assumptions` - the assumptions AND validity regime under which
  the equation holds (e.g. "non-interacting electrons, T = 0, linear response").
  Plain text, semicolon-separated clauses.
- `equations[].variables[].description` - short noun phrase per symbol,
  matching the style of existing entries.
- `equations[].variables[].quantity_id` - link the variable to an entry in
  `quantities` when the symbol denotes a genuine physical quantity:
  - Reuse an existing quantity's `id` when symbol/meaning match.
  - Otherwise append a new quantity to `quantities`: fresh UUIDv4 `id`,
    `symbol` (LaTeX), `name`, `description`, `units` (SI or "dimensionless").
  - Leave `quantity_id` null for indices, dummy/integration variables, and
    generic labels.
- `equations[].references` - the paper where the equation was introduced or is
  canonically presented. Use web search to verify the actual publication; fill
  `authors`, `year`, `title`, `doi`, `url` (prefer the DOI URL). Only add a
  reference you are confident in - an empty list is better than a guess. Never
  invent `pages`.
- complete `quantities` with all symbols from `equations` but do NOT create
  duplicate names/symbols. The `quantities` list has the following item schema:
  ```json
  {
    "id": "UUID",
    "symbol": "\\Gamma",
    "name": "",
    "description": "full two-particle vertex",
    "units": ""
  },
  ```
  notice the double `\` inside the symbol.

## Verify the following against the internet:

`name`, `latex`, `tags`, `description`, `assumptions`, `units`, `references`

## Never modify
- `id`, `px_height`, `created_at`
- never remove or reorder equations or quantities

When unsure about physics content, leave the field empty rather than guessing.
