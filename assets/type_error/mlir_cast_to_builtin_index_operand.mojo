# `pop.cast_to_builtin` casts `!kgen.scalar` storage to its builtin type:
# the pin's verifier rejects an operand that is already an `index`.
# expect: invalid call to 'pop.cast_to_builtin': op operand #0 must be parameterized SIMD vector type, but got 'index'
def main():
    var a = 3
    var bad = __mlir_op.`pop.cast_to_builtin`[_type=__mlir_type.index](
        a.__mlir_index__()
    )
