# Mojito narrows a SIMD element argument to the lane type, wrapping an
# out-of-range literal and a wider runtime value; upstream wants each element
# to already be the lane's scalar.
def main():
    var i = 300
    print(SIMD[DType.uint8, 4](i, 1, 2, 259))
