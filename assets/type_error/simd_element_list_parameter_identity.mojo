# A vector's element list in a parameter position is the application of
# `SIMD`'s `*elems` initializer, as at the pin, not the vector it builds, so
# it is a distinct parameter from the splat of the same lanes.
# expect: type mismatch for variable 'a'
struct Tag[v: SIMD[DType.int32, 2]]:
    def __init__(out self):
        pass


def main():
    var a: Tag[SIMD[DType.int32, 2](3)] = Tag[SIMD[DType.int32, 2](3, 3)]()
