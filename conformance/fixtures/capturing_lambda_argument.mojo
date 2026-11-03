# A run-time parameter typed `def(...) capturing[_]`: both compilers reject
# the removed `capturing` effect at the parse, with the pin's text.
def observe(f: def(x: Int) capturing[_] -> Int, value: Int) -> Int:
    return f(value)

def main():
    var factor = 3
    print(observe(lambda (x: Int) -> Int: x * factor, 5))
