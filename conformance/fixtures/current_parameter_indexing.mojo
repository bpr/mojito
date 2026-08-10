# Current Mojo's compile-time parameter indexing hook is
# `__getitem_param__`, not the earlier `__getitem__` spelling.
@fieldwise_init
struct CurrentPair(Copyable, Deinitable):
    var first: Int

    def __getitem_param__[index: Int](self) -> Int:
        return self.first + index

def main():
    var pair = CurrentPair(7)
    print(pair[0], pair[1])
    print(CurrentPair(9)[0])
