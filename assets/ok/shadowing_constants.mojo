# A module-level `comptime` constant is not a capture: a nested `def` reads it
# without naming it in a capture list, and the read is the constant, not a
# snapshot of a local. Mojito still demands a capture list for a *function*-
# local `comptime` constant, which the pin refuses to let you write — the
# `local-comptime-capture` conformance case.
comptime counter: Int = 1

def main():
    print(counter) # Prints 1
    def inner():
        var local = counter + 3
        print(local) # Prints 4
    inner()
    print(counter) # Prints 1 from outer constant
