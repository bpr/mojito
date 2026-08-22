# Thin retained callables through the native two-word `{invoke, env}` value:
# bare function references stored in locals, passed downward as arguments
# (the non-escaping rule keeps callables out of returns and rebinds), and a
# raising contract invoked indirectly inside `try` — the tagged outcome
# flows through the invoke thunk.
def twice(x: Int) -> Int:
    return x * 2

def add_one(x: Int) -> Int:
    return x + 1

def apply(f: def(x: Int) -> Int, value: Int) -> Int:
    return f(value)

def checked_div(a: Int, b: Int) raises -> Int:
    if b == 0:
        raise Error("division by zero")
    return a // b

def risky_apply(f: def(a: Int, b: Int) raises -> Int, a: Int, b: Int) raises -> Int:
    return f(a, b)

def main():
    var f: def(x: Int) thin -> Int = twice
    print(f(21))
    var g: def(x: Int) thin -> Int = add_one
    print(g(41))
    print(apply(twice, 10))
    print(apply(lambda (x: Int) {} -> Int: x + 5, 10))
    print(apply(f, 6))
    try:
        print(risky_apply(checked_div, 10, 2))
        print(risky_apply(checked_div, 1, 0))
    except e:
        print("caught")
