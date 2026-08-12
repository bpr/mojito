# LayoutTensor is a layout-aware view over a caller-managed buffer: the
# compile-time layout folds into the specialization, and row- versus
# col-major layouts map the same coordinates to different flat offsets.
from std.memory import unsafe_alloc

from layout import IntTuple, Layout, LayoutTensor

def main():
    var data = unsafe_alloc[Scalar[DType.float32]](6)
    var k = 0
    while k < 6:
        data[k] = Scalar[DType.float32](k)
        k += 1
    var t = LayoutTensor[DType.float32, Layout.row_major(2, 3)](data)
    print(t.size())
    print(t.dim(0))
    print(t.dim(1))
    print(t[0, 1])
    print(t[1, 2])
    t[1, 2] = Scalar[DType.float32](99)
    print(data[5])
    var c = LayoutTensor[DType.float32, Layout.col_major(2, 3)](data)
    print(c[0, 1])
    print(c[1, 2])
    data.free()
