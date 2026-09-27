# A `List` of multi-lane vectors reads and stores its elements by subscript:
# the selected `__getitem__`/`__setitem__` clone's symbol carries the
# vector's `DType.int32`, whose `.` never splits the method symbol.
def main():
    var l = List[SIMD[DType.int32, 4]]()
    l.append(SIMD[DType.int32, 4](1, 2, 3, 4))
    print(l[0])
    l[0] = SIMD[DType.int32, 4](5, 6, 7, 8)
    l[0] += 1
    print(l[0], len(l))
