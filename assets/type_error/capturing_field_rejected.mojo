# A `capturing[...]`-annotated callable field is rejected like any other
# `def(...)`-typed field: current Mojo has no callable-value storage positions.
# expect: type of struct field 'callback'
@fieldwise_init
struct Env:
    var callback: def(Int) capturing[_] -> Int

def main():
    var n = 10
    def bump(x: Int) {imm n} -> Int:
        return x + n
    var env = Env(bump)
    print(env.callback(5))
