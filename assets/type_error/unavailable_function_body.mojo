# expect: unexpected function body in @unavailable function declaration
# An `@unavailable` function declares a signature only: its body must be
# `...`.
@unavailable("never")
def g(x: Bool) -> Int:
    return 1


def main():
    print(1)
