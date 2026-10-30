# A `!kgen.pointer` builds only a `Pointer` of its own element type.
# expect: does not convert to
def main():
    var x = 7
    var k = Pointer(to=x)._get_kgen_pointer()
    var q = Pointer[Float64, origin_of(x)](_mlir_value=k)
    print(q[])
