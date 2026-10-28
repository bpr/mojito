# `pop.cast_from_builtin` casts a builtin `index` to `!kgen.scalar<index>`:
# the pin's verifier rejects an operand that is already that storage.
# expect: invalid call to 'pop.cast_from_builtin': op cannot convert to scalar dtype index from '!kgen.scalar<index>'
def main():
    var a = 3
    var bad = __mlir_op.`pop.cast_from_builtin`[
        _type=__mlir_type.`!kgen.scalar<index>`
    ](a._mlir_value)
