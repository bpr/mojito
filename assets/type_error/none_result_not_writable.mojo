# expect: does not conform to trait 'Writable'
# A `-> None` result is the `None` type, which is not `Writable` at the pin.
def k() -> None:
    pass


def main():
    print(k())
