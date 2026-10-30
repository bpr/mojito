# Upstream's `std/_plugin/selector.mojo`, in part: `Plugin` and `_index_lt`,
# verbatim.
# Omitted: `PluginSelector`, the GPU plugin registry only `_overlay.mojo`
# reads, and `TargetPluginSelector`, which selects a plugin from a target's
# `opaque_plugin` payload through `#kgen.get_target_plugin`, `TypeList.of`,
# and `downcast` (roadmap R1694), with the imports only they use.
"""Shared compile-time plugin registration and selection utilities."""


trait Plugin:
    """Defines the identity required by a compile-time plugin registry.

    Each plugin system refines this trait with a parametric member
    `comptime HookFor[target: CompilationTarget]: <Hooks>` naming the hooks it
    uses for `target`. Traits can't take parameters, so this indirection is how
    a plugin's hook trait gets specialized per target: the plugin is a plain
    struct that selects a backend by `name`, and `HookFor[target]` is a
    separate struct parameterized on the target that implements the actual
    hooks.
    """

    comptime name: __mlir_type.`!kgen.string`
    """The stable identifier matched against a target's plugin field."""


def _index_lt[lhs: __mlir_type.index, rhs: __mlir_type.index]() -> Bool:
    """Returns whether raw parameter index `lhs` is less than `rhs`."""
    return __mlir_op.`index.cmp`[pred=__mlir_attr.`#index.cmp_predicate<ult>`](
        lhs, rhs
    )
