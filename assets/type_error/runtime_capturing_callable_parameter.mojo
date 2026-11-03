# A capturing closure has no run-time value of a `capturing[...]` callable
# type: a run-time parameter cannot spell one. The closure passes through a
# compile-time `F: def(...)` binder instead.
# expect: capturing closures cannot be materialized as runtime values (parameter 'f')
def call_rt(f: def(Int) capturing[_] -> Int, a: Int) -> Int:
    return f(a)

def main():
    var k = 10
    def inner(x: Int) {k} -> Int:
        return x + k
    print(call_rt(inner, 1))
