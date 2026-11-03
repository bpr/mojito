# The removed `capturing` effect rejects in a run-time parameter's type: a
# closure passes through a closure-trait binder `F: def(...)` instead.
# expect: the 'capturing' function effect is no longer supported; use a closure capture list like '{var}' or '{mut x}' instead
def call_rt(f: def(Int) capturing[_] -> Int, a: Int) -> Int:
    return f(a)

def main():
    var k = 10
    def inner(x: Int) {k} -> Int:
        return x + k
    print(call_rt(inner, 1))
