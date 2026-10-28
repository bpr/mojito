# expect: cannot infer type parameter 'n'
# A width binder declared `Int` is not solved from a known width: SIMD's
# width parameter is a `SIMDLength`, so the binder is solved only from the
# `SIMDLength(n)` conversion an `Int` width expression spells
# (`assets/ok/simd_int_width_binder_inferred.mojo`), and a literal width
# spells none, as at the pin. Declaring it `SIMDLength` infers it
# (`assets/ok/simd_lane_binders_inferred.mojo`).
def lanes[dt: DType, n: Int](v: SIMD[dt, n]) -> Int:
    return n


def main():
    print(lanes(SIMD[DType.int32, 4](1, 2, 3, 4)))
