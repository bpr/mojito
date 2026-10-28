# expect: cannot infer type parameter 'w'
# `Int(m)` of a `SIMDLength` binder is its lane count, and the width slot's
# implicit `SIMDLength(Int(m))` is `m` itself, which no construction spells:
# an `Int` width binder is not solved from it, as the pin leaves `w`
# unresolved (`assets/ok/simd_int_width_binder_inferred.mojo` solves from
# `Int(m) * 2`).
def show[w: Int](x: SIMD[DType.int32, w]):
    print(w, x)


def f[m: SIMDLength]():
    show(SIMD[DType.int32, Int(m)](1))


def main():
    f[2]()
