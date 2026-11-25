# expect: value of type 'AnyStruct[Int]' is not compatible with value of type 'Int'
# A struct method's annotation that calls an overloaded module function
# selects among every overload, which the pin resolves on demand, so the
# conditional's type arm meets the call's value arm.
def g() -> Int:
    return 1


def g(x: Int) -> Int:
    return x


struct S:
    def m(self) -> Int if True else g():
        return 1


def main():
    pass
