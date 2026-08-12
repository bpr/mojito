# A rank-1 LayoutTensor with an integer dtype: reads and writes go through
# the layout's linear mapping; the view never owns the buffer.
from std.memory import unsafe_alloc

from layout import Layout, LayoutTensor

def main():
    var data = unsafe_alloc[Scalar[DType.int32]](4)
    var k = 0
    while k < 4:
        data[k] = Scalar[DType.int32](10 * k)
        k += 1
    var t = LayoutTensor[DType.int32, Layout.row_major(4)](data)
    print(t.size())
    print(t[3])
    t[0] = Scalar[DType.int32](7)
    print(data[0])
    data.free()
