# expect: type mismatch for variable 'x'
# Two converted arguments are one value only when their constructions are.
struct W(ImplicitlyCopyable):
    var v: Int

    @implicit
    def __init__(out self, value: Int, /):
        self.v = value * 2


struct T[w: W]:
    def __init__(out self):
        pass


def main():
    var x: T[3] = T[4]()
    _ = x^
