# A run-time parameter of a non-`thin` `def(...)` type has the trait's
# `AnyTrait[...]` type, which its `__call__` cannot take as `Self`.
# expect: value has type 'AnyTrait[def(Int) -> Int]'
def via(f: def(Int) -> Int, x: Int) -> Int:
    return f(x)

def main():
    print(1)
