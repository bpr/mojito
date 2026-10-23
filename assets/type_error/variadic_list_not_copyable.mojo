# expect: cannot copy non-Copyable type 'VariadicList[Int, false]'
# A homogeneous collector is upstream's `VariadicList`, which is not
# implicitly copyable.
def show(*xs: Int):
    var l = xs
    print(len(l))


def main():
    show(1, 2, 3)
