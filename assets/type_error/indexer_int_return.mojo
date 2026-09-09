# expect: no '__mlir_index__' candidates have type 'def(self: Offset) thin -> __mlir_type.index'
# Upstream's `Indexer` requires `__mlir_index__(self) -> __mlir_type.index`;
# an `Int` return does not satisfy it.
@fieldwise_init
struct Offset(Indexer):
    var value: Int

    def __mlir_index__(self) -> Int:
        return self.value


def main():
    var values = [3, 7]
    print(values[Offset(1)])
