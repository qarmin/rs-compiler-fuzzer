"""Template categories for the miscompile-fuzzer generator.

Each module exposes a single `generate(rng, idx) -> Program` function, where
`rng` is a `random.Random` seeded deterministically by the caller (see
`../generator.py`) and `idx` is the corpus index (useful for making fn/type
names unique within the emitted source file).

A `Program` is the small dataclass defined in `common.py`: the full `.rs`
source text, plus the stdout the generator independently predicts that source
will print when run correctly. The diff runner uses the prediction as a third
oracle alongside the two (or more) compiled-config outputs it compares against
each other - see `../README.md` for why that matters for ruling out
generator/template bugs vs. real compiler divergences.
"""

from .common import Program

TEMPLATES: dict[str, "callable"] = {}

# Categories registered with `default=False` are intentionally excluded from
# generator.py's default (no `--categories`) rotation - e.g.
# `identity_chain_unsafe`, which the README documents as small and opt-in.
# They remain fully usable via an explicit `--categories <name>`.
NOT_DEFAULT: set[str] = set()


def register(name, default=True):
    def deco(fn):
        TEMPLATES[name] = fn
        if not default:
            NOT_DEFAULT.add(name)
        return fn

    return deco


# Importing the category modules registers them into TEMPLATES via the
# `@register(...)` decorator at module load time.
from . import (  # noqa: E402,F401
    arithmetic,
    bit_intrinsics,
    cell_aliasing,
    closures,
    const_eval,
    control_flow,
    drop_order,
    enum_niche,
    identity_chain,
    identity_chain_unsafe,
    match_branch,
    niche_generics,
    packed_structs,
    provenance,
    reborrow,
    simd_autovec,
    static_merge,
    trait_objects,
    unions,
)
