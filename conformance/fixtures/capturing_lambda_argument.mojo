# A run-time parameter typed `def(...) capturing[_]`: both compilers reject
# it, since a capturing callable has no run-time value of that type (the pin
# has also removed the `capturing` spelling).
def observe(f: def(x: Int) capturing[_] -> Int, value: Int) -> Int:
    return f(value)

def main():
    var factor = 3
    print(observe(lambda (x: Int) -> Int: x * factor, 5))
