# A `!lit.ref`'s origin is part of its type: it builds no `Pointer` of
# another origin.
# expect: does not convert to
def main():
    var x = 7
    var y = 8
    var q = Pointer[Int, origin_of(y)](_mlir_value=__get_mvalue_as_litref(x))
    print(q[])
