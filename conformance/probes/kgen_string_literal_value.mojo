# PROBE: a string literal's `.value` is a `!kgen.string` compile-time value, and a raw list of them is a `ParameterList`'s parameter (roadmap R809).
#
# The pin prints:
#   2
#
# Observed 2026-10-20 against `Mojo 1.2.0.dev2026101705 (46f49943)`.
# Mojito: stops only at `ParameterList.of` (R809), which parses since R806 ("unsupported compile-time expression"); `"None".value` and the `__mlir_type` alias run (`assets/ok/kgen_string_literal_value.mojo`).
# When Mojito runs it, delete it: R1132's fixture covers the spelling.
#
# Run:    mojo run kgen_string_literal_value.mojo
#         cargo run -- run conformance/probes/kgen_string_literal_value.mojo
from std.builtin.variadics import _MLIR

comptime KGENString = __mlir_type.`!kgen.string`


def main():
    comptime names: _MLIR.KGENParamListType[KGENString] = ParameterList.of[
        "None".value, "Some".value
    ].values
    print(ParameterList[names]().size)
