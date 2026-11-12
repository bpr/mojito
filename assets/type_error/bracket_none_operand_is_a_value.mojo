# `None` in a bracket is the `None` value, as at the pin, so a type
# parameter rejects it.
# expect: expected a type, found a value


def main():
    comptime A = List[None]
    print(1)
