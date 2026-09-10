# Upstream's `SIMD.__getitem__` takes a plain `Int`, so an `Indexer` value
# spelled directly as a lane index is rejected ("value passed to 'idx'
# cannot be converted from 'Offset' to 'Int'"). Mojito normalizes any
# `Indexer` through `__mlir_index__` at every subscript, SIMD included, and
# accepts the program. A user type's own `__getitem__` does take an
# `Indexer` upstream — that is `assets/ok/indexer_normalization.mojo`.
@fieldwise_init
struct Offset(Indexer):
    var value: Int

    def __mlir_index__(self) -> __mlir_type.index:
        return self.value.__mlir_index__()


def main():
    var v = SIMD[DType.int, 4](1, 2, 3, 4)
    v[Offset(3)] = 40
    print(v[Offset(3)])
