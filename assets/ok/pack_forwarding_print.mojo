# A pack-keyed `def` forwards its pack whole into `print`, with and without
# the `sep` and `end` keywords.
def outer[*Ts: Writable](*a: *Ts):
    print(*a)
    print(*a, sep=", ", end="!\n")


def main():
    outer(1, "two")
    outer(3.5, True, "x")
