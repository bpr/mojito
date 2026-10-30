# `CurrentPlugin` is the current target's `stdlib_plugin` member, the
# `DefaultPluginHooks` instance every CPU target selects, so each hook reads
# its `PluginHooks` default, and `stdlib_plugin()` names the `default`
# plugin. The math hooks default to `None`, an empty `OptionalReg` over a
# parametric function type, which selects the `else` arm.
from std._plugin import CurrentPlugin, DefaultPlugin, PluginHooks
from std.sys.info import CompilationTarget, stdlib_plugin


def main():
    print(CurrentPlugin.lowers_subvector_intrinsics)
    print(CurrentPlugin.has_generic_address_space)
    print(CurrentPlugin.supports_float64)
    print(CurrentPlugin.flushes_subnormals[DType.float32])
    print(CurrentPlugin._handles_debug_assert)
    print(CompilationTarget.current().stdlib_plugin.lowers_subvector_intrinsics)
    print(stdlib_plugin())
    print(StaticString(DefaultPlugin.name))
    print(conforms_to(CurrentPlugin, PluginHooks))
    comptime if CurrentPlugin.lowers_subvector_intrinsics:
        print("subvector intrinsics")
    else:
        print("element accesses")
    comptime if CurrentPlugin.unary_math_fn["sqrt", DType.float32, 4]:
        print("unary hook")
    else:
        print("no unary hook")
    comptime if CurrentPlugin.binary_math_fn["pow", DType.float64, 1]:
        print("binary hook")
    else:
        print("no binary hook")
