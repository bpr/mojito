# expect: no overload matches
# A LayoutTensor's element dtype is part of its specialized type: storing a
# different dtype's scalar rejects at checking.
from std.memory import unsafe_alloc

from layout import Layout, LayoutTensor

def main():
    var data = unsafe_alloc[Scalar[DType.float32]](2)
    var t = LayoutTensor[DType.float32, Layout.row_major(2)](data)
    t[0] = Scalar[DType.int32](1)
    data.free()
