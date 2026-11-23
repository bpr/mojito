# expect: use of invalidated interior reference 'h' to 'ys["element"]'
# A conditional expression building a ref-field struct over list elements
# holds its arms' loans: growing the list while the result is live
# invalidates it, as it does the bare construction.
struct H[mut: Bool, //, o: Origin[mut=mut]]:
    var r: ref[Self.o] Int

    def __init__(out self, ref[Self.o] s: Int):
        self.r = s


def main():
    var ys: List[Int] = [1, 2]
    var c = True
    var h = H(ys[0]) if c else H(ys[1])
    ys.append(3)
    print(h.r)
