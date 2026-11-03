# The removed `capturing` effect rejects on a compile-time callable
# parameter: a capture-free callable spells `thin`, and a closure passes
# through a closure-trait binder `F: def(...)`.
# expect: the 'capturing' function effect is no longer supported; use a closure capture list like '{var}' or '{mut x}' instead
def apply[f: def(Int) capturing -> Int](x: Int) -> Int:
    return f(x)

def double(x: Int) -> Int:
    return x * 2

def main():
    print(apply[double](3))
