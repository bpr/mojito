# The layout package's runtime core: row/col-major factories, rank/size/
# cosize, direct coordinate-to-linear mapping through the callable Layout,
# equality, and printing.
from layout import IntTuple, Layout

def main():
    var rm = Layout.row_major(2, 3)
    print(rm)
    print(rm.rank())
    print(rm.size())
    print(rm.cosize())
    print(rm(IntTuple(0, 0)))
    print(rm(IntTuple(0, 2)))
    print(rm(IntTuple(1, 0)))
    print(rm(IntTuple(1, 2)))
    var cm = Layout.col_major(2, 3)
    print(cm)
    print(cm(IntTuple(1, 2)))
    print(rm == Layout.row_major(2, 3))
    print(rm == cm)
    var strided = Layout(IntTuple(2, 2), IntTuple(1, 4))
    print(strided.cosize())
    print(strided(IntTuple(1, 1)))
