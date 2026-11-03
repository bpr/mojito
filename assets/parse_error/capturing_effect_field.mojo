# The removed `capturing` effect rejects in a field's function type.
# expect: the 'capturing' function effect is no longer supported; use a closure capture list like '{var}' or '{mut x}' instead
@fieldwise_init
struct Env:
    var callback: def(Int) capturing[_] -> Int

def main():
    var n = 10
    def bump(x: Int) {imm n} -> Int:
        return x + n
    var env = Env(bump)
    print(env.callback(5))
