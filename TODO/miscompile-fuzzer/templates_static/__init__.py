"""Static (deterministic, combinatorial) template categories.

Sibling of `../templates/` (the random/seeded generator), built as a second,
complementary mode per the user's request: no RNG anywhere in this package.
Each module declares an explicit, bounded "scenario matrix" - a plain dict of
`{axis_name: [allowed values]}` - and `generate_all()` enumerates its full
cartesian product deterministically (same input, same corpus, byte for byte,
every time). Adding coverage means adding one value to one axis list; it
never means touching the generation logic. This mirrors the pattern in
~/Projekty/Rust/hodow's `scripts/cpu/generate_tests.py` (a declarative spec
enumerated mechanically into one Rust source per declared combination), not
a random fuzzer - "more certain" in the sense that every generated program is
an intentional, named point in the test matrix, not a sampled one.

Each module exposes a single `generate_all() -> list[Program]` function,
registered via `@register_static("category_name")`.
"""

from templates.common import Program  # noqa: F401 - re-exported for template modules' convenience

STATIC_TEMPLATES: dict[str, "callable"] = {}


def register_static(name):
    def deco(fn):
        STATIC_TEMPLATES[name] = fn
        return fn

    return deco


# Importing the category modules registers them into STATIC_TEMPLATES via the
# `@register_static(...)` decorator at module load time.
from . import (  # noqa: E402,F401
    packed_structs_static,
    provenance_static,
    reborrow_static,
    unions_static,
)
