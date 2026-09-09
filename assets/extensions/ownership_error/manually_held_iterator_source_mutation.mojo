# expect: conflicts with live reference
# A manually held borrowing iterator retains its source loan outside any
# loop: mutating the source while the iterator lives is rejected.
@fieldwise_init
struct StopIteration:
    pass


@fieldwise_init
struct NumbersIter[m: Bool, //, o: Origin[mut=m]]:
    var src: ref[o] List[Int]
    var index: Int

    def __next__(mut self) raises StopIteration -> ref[Self.o] Int:
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
