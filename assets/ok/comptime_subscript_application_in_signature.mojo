# A subscript or a reference-returning getter call in a type argument is the
# getter's application, read through `load_from_mem`: the signature and the
# call spell the same unfolded application, so the conversion is the
# identity, as at the pin.
def mk() -> List[Int]:
    return [2, 1]


def by_call_result(x: SIMD[DType.int8, mk()[0]]) -> Int:
    return len(x)


def by_explicit_getter(x: SIMD[DType.int8, [2, 1].__getitem__(0)]) -> Int:
    return len(x)


def by_call_result_getter(x: SIMD[DType.int8, mk().__getitem__(1)]) -> Int:
    return len(x)


def by_tuple(x: SIMD[DType.int8, (2, 1)[0]]) -> Int:
    return len(x)


struct S[n: Int]:
    comptime w = (Self.n, 1)[0]


def main():
    print(by_call_result(SIMD[DType.int8, mk()[0]](1, 2)))
    print(by_explicit_getter(SIMD[DType.int8, [2, 1].__getitem__(0)](1, 2)))
    print(by_call_result_getter(SIMD[DType.int8, mk().__getitem__(1)](7)))
    print(by_tuple(SIMD[DType.int8, (2, 1)[0]](1, 2)))
    print(S[2].w)
