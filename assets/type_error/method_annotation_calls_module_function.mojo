# expect: value of type 'AnyStruct[Int]' is not compatible with value of type 'Int'
# A struct method's annotation that calls a module function types the call
# against the function's signature, which the pin resolves on demand, so the
# conditional's type arm meets the call's value arm.
def g() -> Int:
    return 1


struct S:
    def m(self, x: Int if True else g()):
        pass


def main():
    pass
