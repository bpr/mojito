# expect: dereference of dangling Pointer
# The pin's default `-D ASSERT` mode skips the element list's arity assert, so
# too few elements read past `elems` and crash at run time.
def main():
    var v = SIMD[DType.int32, 4](1, 2)
    print(v)
