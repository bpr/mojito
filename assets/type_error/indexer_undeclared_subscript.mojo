# expect: invalid call to '__getitem__': no overload matches the supplied arguments
# A struct indexes a collection only by declaring upstream's `Indexer`: an
# `__mlir_index__` method alone does not make it a conformer.
@fieldwise_init
struct Offset(Copyable):
    var value: Int

    def __mlir_index__(self) -> __mlir_type.index:
        return self.value.__mlir_index__()


def main():
    var values = [3, 7]
    print(values[Offset(1)])
