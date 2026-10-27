# expect: lacking evidence to prove correctness
# A dict display over a value whose bounds neither provide nor exclude
# `Deinitable` cannot prove the constructor's `where` clause: the
# conformance is unknown at the symbolic `T`, not violated.
def f[T: Copyable & Hashable & Equatable](x: T):
    var d = {1: x.copy()}

def main():
    f(1)
