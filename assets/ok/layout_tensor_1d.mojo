# A rank-1 LayoutTensor with an integer dtype: reads and writes go through
# the layout's linear mapping; the view never owns the buffer.
from layout import Layout, LayoutTensor

def main():
    var data = UnsafePointer[Scalar[DType.int32]].alloc(4)
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
