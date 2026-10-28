# An `Indexer`'s `__mlir_index__` returns the builtin `index`: an `Int`
# value does not convert to it implicitly, as at the pin.
# expect: type mismatch for return: expected __mlir_type.index, found Int
@fieldwise_init
struct Offset(Indexer):
    var value: Int

    def __mlir_index__(self) -> __mlir_type.index:
        return self.value


def main():
    var values = [3, 7]
    print(values[Offset(1)])
