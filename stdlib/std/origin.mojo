"""Origin (provenance) vocabulary.

`Origin` is declared here, as upstream declares it, with only its `mut`
parameter: Mojito spells no raw MLIR origin value, so the struct has no
constructor and no methods. `ImmOrigin` and `MutOrigin` are its aliases.
`OriginSet` and the untracked/static/unsafe origin spellings are compiler
builtins available implicitly in every program; the linker exports their
identities from this module so explicit `from std.origin import ...`
spellings resolve.
"""

comptime ImmOrigin = Origin[mut=False]
"""Immutable origin reference type."""

comptime MutOrigin = Origin[mut=True]
"""Mutable origin reference type."""


struct Origin[mut: Bool, //](TrivialRegisterPassable):
    """This represents a origin reference for a memory value.

    Parameters:
        mut: Whether the origin is mutable.
    """

    pass
