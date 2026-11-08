# expect: failed to parse POP_FastmathFlagsAttr parameter 'value'
# A `#pop.fmf<…>` attribute must spell a fast-math flag; the pin rejects a
# misspelled one where it is written, before any op reads it.
def bad() -> __mlir_type.`!kgen.deferred`:
    return __mlir_attr.`#pop.fmf<bogus>`


def main():
    var a = Float32(1.5)
    print(Float32(mlir_value=__mlir_op.`pop.add`[fastmathFlags=bad()](a._mlir_value, a._mlir_value)))
