# A `!kgen.pointer` carries no origin, so `Pointer(_mlir_value=)` over one
# needs a bracket that names it.
# expect: failed to infer parameter 'mut'
def main():
    var x = 7
    var k = Pointer(to=x)._get_kgen_pointer()
    var q = Pointer(_mlir_value=k)
    print(q[])
