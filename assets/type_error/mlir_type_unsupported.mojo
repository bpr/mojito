# expect: this MLIR type spelling is outside the supported subset
# `__mlir_type.index`, the raw parameter list
# `__mlir_type[`!kgen.param_list<`, T, `>`]`, and the `!kgen.simd` vector are
# among the MLIR type spellings inside the subset.
@fieldwise_init
struct Offset(Indexer):
    var value: Int
    def __mlir_index__(self) -> __mlir_type.i64:
        return self.value.__mlir_index__()
def main():
    print(1)
