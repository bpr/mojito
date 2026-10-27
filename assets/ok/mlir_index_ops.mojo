# Upstream's `SIMDLength` operator bodies: `__mlir_op` `index` ops over
# `__mlir_type.index` values, the compare's `i1` read through `Bool(value=)`.


def index_ops(a: Int, b: Int):
    var x = a.__mlir_index__()
    var y = b.__mlir_index__()
    print(
        Int(mlir_value=__mlir_op.`index.add`(x, y)),
        Int(mlir_value=__mlir_op.`index.sub`(x, y)),
        Int(mlir_value=__mlir_op.`index.mul`(x, y)),
        Int(mlir_value=__mlir_op.`index.divs`(x, y)),
        Int(mlir_value=__mlir_op.`index.and`(x, y)),
        Int(mlir_value=__mlir_op.`index.shrs`(x, y)),
    )
    print(
        Bool(
            value=__mlir_op.`index.cmp`[
                pred=__mlir_attr.`#index.cmp_predicate<slt>`
            ](x, y)
        ),
        Bool(
            value=__mlir_op.`index.cmp`[
                pred=__mlir_attr.`#index.cmp_predicate<sge>`
            ](x, y)
        ),
    )


def main():
    index_ops(-7, 2)
    index_ops(9, 3)
