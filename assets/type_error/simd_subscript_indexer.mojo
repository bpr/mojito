# expect: type mismatch for index: expected Int, found Offset
# Upstream's builtin `SIMD.__getitem__`/`__setitem__` take a plain `Int`, so an
# `Indexer` value spelled as a lane index is rejected ("value passed to 'idx'
# cannot be converted from 'Offset' to 'Int'"). Every other subscript still
# normalizes a conformer through `__mlir_index__` — that is
# `assets/ok/indexer_normalization.mojo`.
@fieldwise_init
struct Offset(Indexer):
    var value: Int

    def __mlir_index__(self) -> __mlir_type.index:
        return self.value.__mlir_index__()


def main():
    var v = SIMD[DType.int, 4](1, 2, 3, 4)
    v[Offset(3)] = 40
    print(v[Offset(3)])
