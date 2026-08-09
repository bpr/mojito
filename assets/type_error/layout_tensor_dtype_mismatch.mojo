# expect: no overload matches
# A LayoutTensor's element dtype is part of its specialized type: storing a
# different dtype's scalar rejects at checking.
from layout import Layout, LayoutTensor

def main():
    var data = UnsafePointer[Scalar[DType.float32]].alloc(2)
    var t = LayoutTensor[DType.float32, Layout.row_major(2)](data)
    t[0] = Scalar[DType.int32](1)
    data.free()
