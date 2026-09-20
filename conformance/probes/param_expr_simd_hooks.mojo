# PROBE: do symbolic SIMD widths canonicalize (follow-on, NOT implemented by the parameter-expression entry)?
#
# Observed 2026-09-20 against `Mojo 1.1.0.dev2026082605 (dd957314)`:
#   pin:    runs, prints four 7s, four 8s, four zeroes
#   mojito: `Ty::Simd` requires a concrete width
#
# Run:    mojo run param_expr_simd_hooks.mojo
#         cargo run -- run conformance/probes/param_expr_simd_hooks.mojo
#
# When answered: the DType/vector validation entry in `docs/roadmap.md` section 1 migrates `Ty::Simd` onto `ParamExpr`; promote then.
def reordered[dt: DType, n: Int](x: SIMD[dt, n + 1]) -> SIMD[dt, 1 + n]:
    return x


def collected[dt: DType, n: Int](x: SIMD[dt, 2 * n]) -> SIMD[dt, n + n]:
    return x


struct Buffer[length: Int](Copyable, Movable):
    var value: Int

    def __init__(out self):
        self.value = 0

    def zeros(self) -> SIMD[DType.int64, Self.length]:
        return SIMD[DType.int64, Self.length](0)


def main():
    print(reordered[DType.int64, 3](SIMD[DType.int64, 4](7)))
    print(collected[DType.int64, 2](SIMD[DType.int64, 4](8)))
    print(Buffer[4]().zeros())
