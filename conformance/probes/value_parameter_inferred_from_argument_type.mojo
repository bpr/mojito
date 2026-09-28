# A `def` value parameter inferred from an argument's type
# (`def size[n: Int](c: Counter[n])` called as `size(Counter[4](1))`) checks,
# then fails MIR verification: "required compile-time value parameter 'n' is
# missing". The pinned Mojo infers `n = 4` and prints 4; supplying it
# (`size[4](...)`) runs on both backends. Roadmap section 3 tracks it; promote
# this probe to `assets/ok` once it runs.
struct Counter[length: Int](Copyable, Movable):
    var i: Int

    def __init__(out self, i: Int):
        self.i = i


def size[n: Int](c: Counter[n]) -> Int:
    return n


def main():
    print(size(Counter[4](1)))
