# A manually held borrowing iterator keeps its source alive outside any loop
# but borrows nothing exclusively: appending to the source while the iterator
# lives runs, and the next element is read from the grown list (R1160).
@fieldwise_init
struct StopIteration:
    pass


@fieldwise_init
struct NumbersIter[m: Bool, //, o: Origin[mut=m]]:
    var src: ref[o] List[Int]
    var index: Int

    def __next__(
        mut self,
    ) raises StopIteration -> ref[Self.o._get_owned_interior["element"]] Int:
        if self.index >= len(self.src):
            raise StopIteration()
        var r = self.index
        self.index += 1
        return self.src[r]


def main():
    var xs: List[Int] = [1, 2, 3]
    ref src = xs
    var it = NumbersIter(src, 0)
    xs.append(4)
    try:
        print(it.__next__())
    except StopIteration:
        pass
