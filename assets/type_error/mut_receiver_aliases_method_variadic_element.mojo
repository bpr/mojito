# expect: aliasing values passed mutably to 'self' argument and passed immutably to 'rest' argument in 'm' call
# A method's positional collector gathers each argument under its own name:
# `s.v` read into `*rest` overlaps the `mut self` receiver, as at the pin.
struct S:
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def m(mut self, *rest: Int) -> Int:
        return self.v


def main():
    var s = S(1)
    print(s.m(s.v))
