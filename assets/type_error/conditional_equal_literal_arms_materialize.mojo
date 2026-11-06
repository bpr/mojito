# expect: expected Int8, found Int
# Two equal literal arms merge at `IntLiteral[1]`, which a run-time
# conditional materializes to `Int`; a call argument is no contextual
# site, so the `Int` does not convert to `Int8`.
def k(x: Int8):
    print(x)


def main():
    var c = True
    k(1 if c else 1)
