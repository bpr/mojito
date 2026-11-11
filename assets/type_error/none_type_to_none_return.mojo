# expect: type mismatch for return: expected None, found NoneType
# `NoneType` does not convert back to the `None` type, as at the pin.
def j() -> None:
    return NoneType()


def main():
    j()
