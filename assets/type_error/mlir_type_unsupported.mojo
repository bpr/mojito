# expect: only '__mlir_type.index' is accepted
# `__mlir_type.index` is the one MLIR type spelling inside the subset.
@fieldwise_init
struct Offset(Indexer):
    var value: Int
    def __mlir_index__(self) -> __mlir_type.i64:
        return self.value.__mlir_index__()
def main():
    print(1)
