# expect: invalid call to '__getitem__'
# `SIMD`'s declared `__getitem__` and `__setitem__` take `idx: Int`, which an
# `Indexer` value does not convert to, so a lane store resolves its getter and
# rejects the index as at the pin ("value passed to 'idx' cannot be converted
# from 'Offset' to 'Int'").
@fieldwise_init
struct Offset(Indexer):
    var value: Int

    def __mlir_index__(self) -> __mlir_type.index:
        return self.value.__mlir_index__()


def main():
    var v = SIMD[DType.int, 4](1, 2, 3, 4)
    v[Offset(3)] = 40
    print(v[Offset(3)])
