# PROBE: `@unavailable` marks a declaration whose every use is an error.
#
# The pin runs it and prints:
#   2
#
# and rejects a call `f(1)` with the decorator's message ("use g
# instead"). Mojito rejects the declaration: "decorator '@unavailable' on a
# function" is unsupported (R1162).
@unavailable("use g instead")
def f(x: Int) -> Int:
    ...

def g(x: Int) -> Int:
    return x + 1

def main():
    print(g(1))
