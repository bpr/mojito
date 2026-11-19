"""Origin (provenance) vocabulary.

`Origin` is declared with upstream's header, its `mut` parameter and the raw
MLIR origin value `_mlir_origin: _lit_origin_type_of_mut[mut]`; the struct
has no constructor and no methods. `_lit_origin_type_of_mut` is declared as
upstream declares it. `ImmOrigin` and `MutOrigin` are its aliases.
`OriginSet` and the untracked/static/unsafe origin spellings are compiler
builtins available implicitly in every program; the linker exports their
identities from this module so explicit `from std.origin import ...`
spellings resolve.
"""

comptime ImmOrigin = Origin[mut=False]
"""Immutable origin reference type."""

comptime MutOrigin = Origin[mut=True]
"""Mutable origin reference type."""

comptime _lit_origin_type_of_mut[mut: Bool] = __mlir_type[
    `!lit.origin<`, mut._mlir_value, `>`
]


struct Origin[mut: Bool, _mlir_origin: _lit_origin_type_of_mut[mut], //](
    TrivialRegisterPassable
):
    """This represents a origin reference for a memory value.

    Parameters:
        mut: Whether the origin is mutable.
        _mlir_origin: The raw MLIR origin value.
    """

    pass
