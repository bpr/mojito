# expect: invalid redefinition of 'd'
# A struct's parameters are in scope in every method's parameter scope, so a
# method's compile-time parameter may not reuse one's name.
struct H[d: DType]:
    def __init__(out self):
        pass

    def f[d: Int](self):
        print(d)

def main():
    H[DType.int8]().f[3]()
