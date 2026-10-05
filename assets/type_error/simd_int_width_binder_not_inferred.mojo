# expect: cannot infer type parameter 'n'
# A width binder declared `Int` is not solved from a vector argument: SIMD's
# width parameter is a `SIMDLength`, so an `Int` binder in that slot is a
# conversion the pin leaves unresolved. Declaring it `SIMDLength` infers it
# (`assets/ok/simd_lane_binders_inferred.mojo`).
def lanes[dt: DType, n: Int](v: SIMD[dt, n]) -> Int:
    return n


def main():
    print(lanes(SIMD[DType.int32, 4](1, 2, 3, 4)))
