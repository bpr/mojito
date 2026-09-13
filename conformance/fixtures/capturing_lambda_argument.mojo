# A capturing lambda passed as a runtime argument to a `capturing[_]`
# contract: Mojito binds it, while the pin can only take a capturing callable
# as a compile-time parameter and rejects a lambda even there.
def observe(f: def(x: Int) capturing[_] -> Int, value: Int) -> Int:
    return f(value)

def main():
    var factor = 3
    print(observe(lambda (x: Int) -> Int: x * factor, 5))
