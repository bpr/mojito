# `Int / Int` is upstream's `SIMD.__truediv__` (`pop.div`): it truncates
# toward zero and stays an `Int`, so `7 / -2` is `-3`. Only an `IntLiteral`
# pair divides into a float, so `7 / 2` is `3.5`.
def main():
    var a = 7
    var b = -2
    print(a / b, 7 / 2)
