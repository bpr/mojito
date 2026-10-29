# A value `__getitem__` result passed as an argument is a temporary, not a
# place: `String(c[0])` reads a user getter once, and a `SIMD` lane, whose
# subscripts are the declared `SIMD.__getitem__`/`__setitem__`, reads, writes
# back through `bump(v[i])`, and augments in a generic `mut` parameter.
struct Cell:
    var x: Int32

    def __init__(out self):
        self.x = 1

    def __getitem__(self, i: Int) -> Int32:
        print("get")
        return self.x


def bump(mut x: Int32):
    x += 40


def add_last[dt: DType, w: Int](mut v: SIMD[dt, w]):
    v[w - 1] += v[0]


def main():
    var c = Cell()
    print(String(c[0]))
    var v = SIMD[DType.int32, 4](1, 2, 3, 4)
    print(String(v[0]))
    var i = 3
    bump(v[i])
    print(v)
    var w = SIMD[DType.int64, 4](5, 6, 7, 8)
    add_last[DType.int64, 4](w)
    print(w)
