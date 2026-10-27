# A `comptime if` or `where` clause comparing a scalar `SIMD` value
# parameter decides by the parameter's value: a one-lane comparison mask
# reads as its `__bool__`.

def g[x: Scalar[DType.uint8]]():
    comptime if x < 10:
        print("lt then")
    else:
        print("lt else")
    comptime if x == 3:
        print("eq then")
    else:
        print("eq else")

def f[y: Scalar[DType.float32]]():
    comptime if y >= 1.5:
        print("ge then")
    else:
        print("ge else")

def w[v: SIMD[DType.int32, 1]]():
    comptime if v != 7:
        print("ne")
    else:
        print("seven")

def h[x: Scalar[DType.int8]]() where x > 0:
    print("positive", x)

def main():
    g[3]()
    g[30]()
    f[2.0]()
    f[0.5]()
    w[7]()
    w[8]()
    h[5]()
