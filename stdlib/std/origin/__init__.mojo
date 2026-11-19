"""Defines Mojo's origin types.

An origin is a compile-time value that names the variable a reference
borrows from, and whether that reference permits mutation. `Origin` is
declared with upstream's header, its `mut` parameter and the raw MLIR origin
value `_mlir_origin: _lit_origin_type_of_mut[mut]`. The named origins
(`ImmStaticOrigin`, `UntrackedOrigin`, `AnyOrigin`, `UnsafeAnyOrigin`, and
their `Imm`/`Mut` aliases) are `Origin` values built from a raw
`!lit.origin` attribute, and `OriginSet` is the raw `!lit.origin.set` type,
as upstream declares them.
"""

comptime ImmOrigin = Origin[mut=False]
"""Immutable origin reference type."""

comptime MutOrigin = Origin[mut=True]
"""Mutable origin reference type."""

comptime AnyOrigin[*, mut: Bool] = Origin[
    _mlir_origin=__mlir_attr[
        `#lit.any.origin : !lit.origin<`, +mut._mlir_value, `>`
    ],
]()
"""An origin that might access any memory value.

Parameters:
    mut: Whether the origin is mutable.
"""

comptime ImmutAnyOrigin = AnyOrigin[mut=False]
"""The immutable origin that might access any memory value."""

comptime MutAnyOrigin = AnyOrigin[mut=True]
"""The mutable origin that might access any memory value."""

comptime UnsafeAnyOrigin[*, mut: Bool] = Origin[
    _mlir_origin=__mlir_attr[
        `#lit.any.origin : !lit.origin<`, +mut._mlir_value, `>`
    ],
]()
"""The universal origin: an unsafe origin that might alias any memory value.

Parameters:
    mut: Whether the origin is mutable.
"""

comptime ImmUnsafeAnyOrigin = UnsafeAnyOrigin[mut=False]
"""The immutable universal origin that might alias any memory value."""

comptime MutUnsafeAnyOrigin = UnsafeAnyOrigin[mut=True]
"""The mutable universal origin that might alias any memory value."""

comptime UntrackedOrigin[*, mut: Bool] = Origin[
    _mlir_origin=__mlir_attr[
        `#lit.origin.union<> : !lit.origin<`,
        +mut._mlir_value,
        `>`,
    ],
]()
"""An origin the lifetime checker does not track, because it aliases no
existing value.

Parameters:
    mut: Whether the origin is mutable.
"""

comptime ImmUntrackedOrigin = UntrackedOrigin[mut=False]
"""An immutable origin the lifetime checker does not track."""

comptime MutUntrackedOrigin = UntrackedOrigin[mut=True]
"""A mutable origin the lifetime checker does not track."""

# Static constants are a named subset of the global origin.
comptime ImmStaticOrigin = Origin[
    _mlir_origin=__mlir_attr[
        `#lit.origin.field<`,
        `#lit.static.origin : !lit.origin<false>`,
        `, "__constants__"> : !lit.origin<false>`,
    ]
]()
"""An origin for strings and other always-immutable static constants."""

comptime OriginSet = __mlir_type.`!lit.origin.set`
"""A set of origin parameters."""


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

    @always_inline("builtin")
    def __init__(out self):
        """Construct an Origin."""
        pass
