# expect: 'pop.mul' op operand #0 must be SIMD type whose element type is integer or float dtype
# `SIMD.__mul__` asserts nothing of its dtype: its `pop.mul` rejects a `bool`
# mask in each instance, as the op verifier does at the pin.
def main():
    var b = SIMD[DType.bool, 2](True, False)
    print(b * b)
