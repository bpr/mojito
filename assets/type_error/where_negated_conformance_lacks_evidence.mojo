# expect: lacking evidence to prove correctness
# A negated conformance over a parameter whose bound does not decide it is
# unproven in the generic body, however the instance would answer.
def g[T: AnyType]() where not conforms_to(T, Copyable):
    print("g")

def f[T: AnyType]():
    g[T]()

def main():
    f[Int]()
