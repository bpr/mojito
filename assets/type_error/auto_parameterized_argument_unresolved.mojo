# expect: invalid call to 'f': value passed to 'x' cannot be converted from 'B' to 'W[x.v]', it depends on an unresolved parameter 'x.v'
# An argument whose type does not solve the binder a bare parametric
# parameter type declares is rejected, naming the binder as the pin does.
struct W[v: Int](ImplicitlyCopyable, TrivialRegisterPassable):
    def __init__(out self):
        pass


struct B(ImplicitlyCopyable, TrivialRegisterPassable):
    var n: Int

    def __init__(out self, n: Int):
        self.n = n


def f(x: W) -> Int:
    return x.v


def main():
    print(f(B(3)))
