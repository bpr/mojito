# expect: type mismatch for comptime 'LABEL': expected String, found Int
# An annotated `comptime` binds at its declared type, so a literal that does
# not convert to the annotation is rejected rather than bound as an `Int`.
comptime LABEL: String = 1


def main():
    print(LABEL)
