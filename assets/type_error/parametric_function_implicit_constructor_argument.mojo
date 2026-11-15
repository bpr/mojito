# A parametric function converts to no parameter of a parametric function
# type, so `Optional`'s implicit constructor rejects its argument as a
# runtime closure.
# expect: cannot use parametric function as a runtime closure: parameter 'd' of type 'DType' is not bound
def twice[d: DType, w: Int, //](x: SIMD[d, w]) -> SIMD[d, w]:
    return x + x


def main():
    var o: Optional[def[d: DType, w: Int, //](SIMD[d, w]) thin -> SIMD[d, w]] = twice
    print(Bool(o))
