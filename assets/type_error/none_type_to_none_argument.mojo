# expect: type mismatch for argument 'x' to 'f': expected None, found NoneType
# A `NoneType` argument does not convert to a `None` parameter, as at the pin.
def f(x: None):
    pass


def main():
    f(NoneType())
