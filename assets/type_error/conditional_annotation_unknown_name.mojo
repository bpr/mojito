# expect: Undefined variable 'Undeclared'
# An annotation the compile-time builder cannot read reports the name it
# reads that declares nothing, not the value binder `n`, which is no runtime
# variable.
def f[n: Int](x: Int if n > 0 else Undeclared):
    pass


def main():
    f[1](2)
